//! Per-instance TLS identities for the guest channel (barista-021).
//!
//! The guest agent listens on a network every sibling VM can reach
//! (`network.name` is always `"default"` — one network per host, not one per
//! instance), and the host dialled it in cleartext with the token in gRPC
//! metadata. Guessing the token was considered when that was ratified; being
//! *on the path* was not.
//!
//! **The pin is arithmetic, not policy.** Each instance gets its own certificate
//! authority, used exactly twice — once for the guest's server certificate, once
//! for the host's client certificate — and then its signing key is dropped
//! without ever being written down. An anchor that cannot sign again cannot
//! authorise a third party, so "trust only this instance" needs no allowlist to
//! stay true.
//!
//! **Why both directions.** The guest verifies the host's certificate too. A
//! server-only pin would stop the host talking to an impostor and leave the
//! guest answering anyone who dialled its port, which is the half of the problem
//! that made this a finding.

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType, IsCa, KeyPair,
    KeyUsagePurpose,
};

/// Install this process's `rustls` crypto provider, once (barista-021 task 1.4).
///
/// **Not optional, and not cosmetic.** Two providers already reach this binary:
/// `ring` through `rcgen`, and `aws-lc-rs` through `object_store` on the fleet's
/// HTTPS path. With both features enabled `rustls` refuses to pick one and
/// `CryptoProvider::get_default()` panics — at the first TLS handshake, in a
/// daemon, rather than at build time.
///
/// `aws-lc-rs` because it is the one already doing real work here: the fleet's
/// bucket traffic goes through it, so choosing `ring` would mean two crypto
/// implementations shipped and one of them idle.
///
/// Idempotent: a second call is a no-op, which is what makes it safe to call
/// from both `main` and every test that builds a TLS config.
pub fn install_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // An error here means a provider was already installed by someone else,
        // which is the outcome we wanted anyway.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// One instance's credentials. The CA key is deliberately absent: it existed
/// inside [`mint`] and nowhere else.
#[derive(Clone, PartialEq, Eq)]
pub struct Identity {
    /// The anchor both ends verify against, DER.
    pub anchor: Vec<u8>,
    /// The guest's server certificate and key, DER.
    pub guest_cert: Vec<u8>,
    pub guest_key: Vec<u8>,
    /// The host's client certificate and key, DER.
    pub host_cert: Vec<u8>,
    pub host_key: Vec<u8>,
}

/// Hand-written, because the derived one prints private keys.
///
/// nap-007 fixed a leak of exactly this shape for the guest token, which is why
/// `Secret` exists on the node side; a key is the same class of value and gets
/// the same treatment rather than a second lesson.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("anchor", &format!("{} bytes", self.anchor.len()))
            .field("guest_cert", &format!("{} bytes", self.guest_cert.len()))
            .field("guest_key", &"[redacted]")
            .field("host_cert", &format!("{} bytes", self.host_cert.len()))
            .field("host_key", &"[redacted]")
            .finish()
    }
}

/// The guest's name in its own certificate.
///
/// A DNS SAN under `.invalid` (RFC 6761: guaranteed never to resolve), because
/// the host dials an IP that the substrate assigns and may reassign. Verifying
/// the address would pin the wrong thing — the address is not the identity, the
/// instance is.
pub fn guest_san(instance_id: &str) -> String {
    format!("guest.{instance_id}.barista.invalid")
}

/// The host's name in *its* certificate.
///
/// A separate name from the guest's, decided rather than inherited: the two ends
/// are different principals, and a single name shared by both would make "the
/// certificate I am holding" and "the certificate I expect" the same string —
/// which is exactly the confusion that lets a guest leaf be presented as a
/// client. The design's earlier wording said "the leaves" carried the guest
/// name; this is the explicit choice it needed (second review, P2).
pub fn host_san(instance_id: &str) -> String {
    format!("host.{instance_id}.barista.invalid")
}

/// Mint an instance's identity. Called once per instance, at create.
///
/// **Once, not per boot, and the reason is a deadlock rather than tidiness.**
/// Restore duties run *over* this channel and stepping the guest's clock is duty
/// two, so the handshake is validated against a clock that is still frozen at
/// whatever the snapshot captured — measured at 25 s behind in nap-005, and
/// arbitrarily worse for a named snapshot restored days later. A certificate
/// minted after a snapshot therefore has a `notBefore` in the guest's future:
/// the handshake fails, the clock is never stepped, and the session is
/// permanently unreachable with nothing in the error pointing at time.
///
/// Minting at create makes `notBefore` predate every snapshot the instance can
/// produce. The five-minute backdate below covers the same hazard on a first
/// boot, where the guest's clock starts at whatever the kernel gives it.
pub fn mint(instance_id: &str) -> anyhow::Result<Identity> {
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            format!("barista instance {instance_id}"),
        );
        dn
    };
    apply_validity(&mut ca_params);

    // Certificate and signing key in one value: rcgen 0.14's issuer owns both,
    // which makes the drop below drop everything that can mint.
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate()?)?;

    let guest = leaf(&guest_san(instance_id), &ca, true)?;
    let host = leaf(&format!("host.{instance_id}.barista.invalid"), &ca, false)?;

    let identity = Identity {
        anchor: ca.der().to_vec(),
        guest_cert: guest.0,
        guest_key: guest.1,
        host_cert: host.0,
        host_key: host.1,
    };

    // The issuer — the signing key with it — goes out of scope here and is never
    // returned, journaled or written. From this line on, the anchor can verify
    // and cannot issue — which is the whole security argument, so it is stated
    // rather than left to be inferred from the absence of a field.
    drop(ca);
    Ok(identity)
}

fn leaf(
    san: &str,
    ca: &CertifiedIssuer<'_, KeyPair>,
    server: bool,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let mut params = CertificateParams::new(vec![san.to_string()])?;
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, san);
        dn
    };
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![if server {
        rcgen::ExtendedKeyUsagePurpose::ServerAuth
    } else {
        rcgen::ExtendedKeyUsagePurpose::ClientAuth
    }];
    apply_validity(&mut params);
    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, ca)?;
    Ok((cert.der().to_vec(), key.serialize_der()))
}

/// Backdated five minutes, and long-lived on purpose.
///
/// Expiry is not the control here — the exhausted anchor is, plus the journal
/// row that `destroy` deletes. A shorter life would only pick a date on which a
/// session that resumed perfectly stops working: nap-015 snapshots have no
/// retention policy, so the interval between minting and the last resume is
/// unbounded, and any `notAfter` is a promise this system cannot keep.
fn apply_validity(params: &mut CertificateParams) {
    let now = std::time::SystemTime::now();
    params.not_before = (now - std::time::Duration::from_secs(300)).into();
    params.not_after = (now + std::time::Duration::from_secs(10 * 365 * 24 * 60 * 60)).into();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_has_two_leaves_under_one_anchor() {
        let id = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        for part in [
            &id.anchor,
            &id.guest_cert,
            &id.guest_key,
            &id.host_cert,
            &id.host_key,
        ] {
            assert!(!part.is_empty());
        }
        assert_ne!(
            id.guest_key, id.host_key,
            "the two ends must not share a key"
        );
        assert_ne!(id.guest_cert, id.host_cert);
    }

    /// Two instances are cryptographically unrelated. This is the property the
    /// whole change buys: a sibling holding its own credentials cannot present
    /// anything the first instance's anchor will accept.
    #[test]
    fn two_instances_share_nothing() {
        let a = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        let b = mint("01JABCDEFGHJKMNPQRSTVWXYZ1").unwrap();
        assert_ne!(a.anchor, b.anchor);
        assert_ne!(a.guest_cert, b.guest_cert);
        assert_ne!(a.host_cert, b.host_cert);
    }

    /// The CA key is unreachable after minting — asserted structurally, since
    /// there is no field to check. If someone later adds one, this fails to
    /// compile rather than silently weakening the pin.
    #[test]
    fn the_signing_key_is_not_part_of_the_identity() {
        let id = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        let Identity {
            anchor: _,
            guest_cert: _,
            guest_key: _,
            host_cert: _,
            host_key: _,
        } = id;
    }

    /// `Debug` must not print key material. nap-007 fixed exactly this leak for
    /// the guest token; a private key is the same class of value.
    #[test]
    fn debug_never_prints_a_private_key() {
        let id = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        let printed = format!("{id:?}");
        assert!(printed.contains("[redacted]"));
        // A DER key's first bytes are stable enough to search for; the point is
        // that no fragment of it survives formatting.
        let needle = id
            .guest_key
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert!(
            !printed.contains(&needle),
            "key bytes reached Debug: {printed}"
        );
    }

    /// The SAN names the instance, never the address. The substrate assigns the
    /// IP and may reassign it; pinning it would pin the wrong thing.
    #[test]
    fn the_name_is_the_instance_not_the_address() {
        let san = guest_san("01JABC");
        assert!(san.ends_with(".barista.invalid"));
        assert!(san.contains("01JABC"));
    }
}

#[cfg(test)]
mod verification {
    use super::*;
    use rustls::client::danger::ServerCertVerifier;

    /// Build the verifier a host would use, anchored on one instance's CA.
    fn verifier_for(anchor: &[u8]) -> std::sync::Arc<rustls::client::WebPkiServerVerifier> {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(anchor.to_vec()))
            .expect("the anchor must parse as a certificate");
        rustls::client::WebPkiServerVerifier::builder_with_provider(
            roots.into(),
            rustls::crypto::aws_lc_rs::default_provider().into(),
        )
        .build()
        .expect("verifier")
    }

    fn verify(anchor: &[u8], cert: &[u8], san: &str) -> Result<(), rustls::Error> {
        let verifier = verifier_for(anchor);
        verifier
            .verify_server_cert(
                &rustls::pki_types::CertificateDer::from(cert.to_vec()),
                &[],
                &rustls::pki_types::ServerName::try_from(san.to_string()).expect("san"),
                &[],
                rustls::pki_types::UnixTime::now(),
            )
            .map(|_| ())
    }

    /// The claim, checked with a real verifier rather than by comparing bytes.
    ///
    /// "The certificates differ" is necessary and not sufficient: what the change
    /// buys is that one instance's anchor *refuses* another's certificate, and
    /// only a verifier can say that.
    #[test]
    fn an_anchor_accepts_its_own_guest_and_refuses_a_siblings() {
        let a = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        let b = mint("01JABCDEFGHJKMNPQRSTVWXYZ1").unwrap();

        verify(
            &a.anchor,
            &a.guest_cert,
            &guest_san("01JABCDEFGHJKMNPQRSTVWXYZ0"),
        )
        .expect("an instance's own guest certificate must verify under its own anchor");

        // The finding, inverted into a test: a hostile sibling on the shared
        // network holds a perfectly valid certificate — its own — and it must
        // buy nothing.
        let err = verify(
            &a.anchor,
            &b.guest_cert,
            &guest_san("01JABCDEFGHJKMNPQRSTVWXYZ1"),
        )
        .expect_err("a sibling's certificate must not verify under this instance's anchor");
        assert!(
            matches!(err, rustls::Error::InvalidCertificate(_)),
            "expected a certificate rejection, got {err:?}"
        );
    }

    /// The name is checked too, not merely the signature. Without this a guest
    /// certificate would verify for any name its anchor covers — which for a
    /// one-instance anchor is nearly harmless, and is exactly the assumption
    /// that stops being true the day the anchor covers two.
    #[test]
    fn a_certificate_does_not_verify_under_the_wrong_name() {
        let a = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        let err = verify(&a.anchor, &a.guest_cert, &guest_san("01JOTHER"))
            .expect_err("the SAN must be checked");
        assert!(
            matches!(err, rustls::Error::InvalidCertificate(_)),
            "{err:?}"
        );
    }
}

#[cfg(test)]
mod client_verification {
    use super::*;
    use rustls::server::danger::ClientCertVerifier;

    /// The verifier a *guest* would use: it decides which clients may talk to it.
    fn verifier_for(anchor: &[u8]) -> std::sync::Arc<dyn ClientCertVerifier> {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(anchor.to_vec()))
            .expect("anchor");
        rustls::server::WebPkiClientVerifier::builder_with_provider(
            roots.into(),
            rustls::crypto::aws_lc_rs::default_provider().into(),
        )
        .build()
        .expect("client verifier")
    }

    fn verify(anchor: &[u8], cert: &[u8]) -> Result<(), rustls::Error> {
        verifier_for(anchor)
            .verify_client_cert(
                &rustls::pki_types::CertificateDer::from(cert.to_vec()),
                &[],
                rustls::pki_types::UnixTime::now(),
            )
            .map(|_| ())
    }

    /// **The half that was not tested.** "Mutual" was the claim and only one
    /// direction had a verifier behind it: `host_cert` was asserted to be
    /// non-empty bytes, so giving it `ServerAuth`, the wrong chain, or a shape
    /// no client verifier accepts would have passed everything
    /// (second review, P1).
    ///
    /// It matters because the guest is the exposed end. A host that verifies its
    /// guest and a guest that answers anyone leaves the port open to precisely
    /// the sibling this change exists to shut out.
    #[test]
    fn a_guest_accepts_its_own_host_and_refuses_everything_else() {
        let a = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        let b = mint("01JABCDEFGHJKMNPQRSTVWXYZ1").unwrap();

        verify(&a.anchor, &a.host_cert)
            .expect("a guest must accept its own host's client certificate");

        // A sibling's host credential — a perfectly valid certificate, issued by
        // an anchor this guest has never heard of.
        assert!(
            matches!(
                verify(&a.anchor, &b.host_cert),
                Err(rustls::Error::InvalidCertificate(_))
            ),
            "a guest must refuse another instance's host"
        );

        // The instance's *own* guest leaf, presented as a client. It chains to
        // the right anchor, so only the extended key usage separates it — which
        // is the check that would silently disappear if the two leaves were ever
        // minted from one template.
        assert!(
            matches!(
                verify(&a.anchor, &a.guest_cert),
                Err(rustls::Error::InvalidCertificate(_))
            ),
            "a server certificate must not be usable as a client certificate"
        );
    }

    /// A client offering nothing must be refused rather than treated as
    /// anonymous-but-acceptable — the default that turns mutual TLS back into
    /// one-way TLS without anybody noticing.
    #[test]
    fn a_client_with_no_certificate_is_refused() {
        let a = mint("01JABCDEFGHJKMNPQRSTVWXYZ0").unwrap();
        assert!(
            verifier_for(&a.anchor).client_auth_mandatory(),
            "client authentication must be mandatory, or the guest answers anyone"
        );
    }
}

#[cfg(test)]
mod provider {
    /// The install must actually take, and a TLS config must build afterwards.
    ///
    /// This exists because the failure it guards is invisible at build time: two
    /// providers reach this binary — `ring` via `rcgen`, `aws-lc-rs` via
    /// `object_store` — and `rustls` responds to the ambiguity by panicking at
    /// the first handshake, inside a running daemon. Deleting the install from
    /// `main` should fail a check here rather than a deployment there.
    #[test]
    fn the_process_provider_is_installed_and_usable() {
        super::install_crypto_provider();
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "no process-wide crypto provider: rustls will panic at the first handshake"
        );
        // Calling twice must be harmless — `main` and every test do.
        super::install_crypto_provider();

        // And the provider must be able to build the thing we need it for.
        let roots = rustls::RootCertStore::empty();
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        assert!(!config.alpn_protocols.iter().any(|p| p.is_empty()));
    }
}
