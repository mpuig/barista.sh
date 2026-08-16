//! The workload's published endpoint, ridden on the substrate's ingress
//! (barista-040).
//!
//! One ingress object per instance, named after its sandbox: `match
//! {advertised host, allocated port}` → `target {sandbox name, guest port}`.
//! The object — not the journal — is the mapping's source of truth: it
//! survives standby/restore and the cold-boot delete-and-recreate untouched,
//! which is what makes the address sticky, and a second copy of the fact
//! would be a disagreement waiting to happen (the barista-030 no-cache rule).
//!
//! Two phases, because the substrate demands it (measured live: `POST
//! /ingresses` answers `400 instance_not_found` for a target that does not
//! exist yet, so ingress-before-sandbox is not an option): [`planned_port`]
//! picks the listener *before* the sandbox — the number has to ride into the
//! guest as `PORT` at create — and [`publish`] writes the object right after
//! the sandbox exists.
//!
//! The gap between the two is where two concurrent creates used to collide:
//! both listed the same free port, both planned it, and the loser's publish
//! took the substrate's 409 — which failed its create *terminally*, because a
//! FAILED instance is not retried by anything (barista-cloud saw this as one
//! lost worker per concurrent fan-out; barista.sh#46). Two layers close it,
//! in the order that matters:
//!
//! 1. [`PortReservations`] — the ports this agent has planned but not yet
//!    published, unioned into "used" when picking. A node allocates only for
//!    itself, so this alone settles every same-node race, which is all of
//!    them in practice, and it costs one lock rather than serializing creates.
//! 2. The caller's bounded re-plan, for the residual the reservation cannot
//!    see (another agent against the same substrate, an operator's own
//!    ingress): a lost publish rolls the sandbox back and plans afresh —
//!    convergence over passes, the reconciler's ordinary shape, now actually
//!    implemented rather than only promised.
//!
//! Nothing here forwards a byte. Barista chooses a port and reports an
//! address; the traffic is the substrate's (ADR-001 v2 §13.7).

use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use tracing::warn;

use super::client::{
    CreateIngressRequest, Error as ClientError, HypemanClient, Ingress, IngressMatch, IngressRule,
    IngressTarget,
};
use crate::ids::InstanceId;
use crate::runtime::{Result, RuntimeError};

/// What a node needs to publish workloads, or absent to publish nothing —
/// the fleet's laptop-mode pattern: not a flag, the absence of configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressConfig {
    /// The host the outside dials this node at. The operator's claim — Barista
    /// cannot discover how the world routes to this machine, exactly as
    /// `--fleet-advertise` already works — and the host half of every address
    /// this node reports.
    pub advertise_host: String,
    /// Listener ports the allocator may hand out.
    pub ports: RangeInclusive<u16>,
}

impl IngressConfig {
    /// Validate at the boundary where operator input arrives (the
    /// `hypeman::config` rule).
    ///
    /// The host must be bare — no scheme, port, path or whitespace — because
    /// it is concatenated into `host:port`: a host smuggling its own port
    /// would produce an address that parses as a different host entirely.
    pub fn new(
        advertise_host: impl Into<String>,
        ports: RangeInclusive<u16>,
    ) -> anyhow::Result<Self> {
        let advertise_host = advertise_host.into();
        anyhow::ensure!(
            !advertise_host.is_empty(),
            "the ingress advertise host must not be empty"
        );
        anyhow::ensure!(
            !advertise_host.contains("://")
                && !advertise_host.contains('/')
                && !advertise_host.contains(char::is_whitespace)
                // A bare IPv6 literal contains colons but is not host:port;
                // nobody publishes on one today, and refusing it beats
                // reporting `::1:30000`, which every parser reads wrong.
                && !advertise_host.contains(':'),
            "the ingress advertise host must be a bare host (no scheme, port or path): \
             {advertise_host:?}"
        );
        anyhow::ensure!(
            !ports.is_empty(),
            "the ingress port range is empty: {}-{}",
            ports.start(),
            ports.end()
        );
        anyhow::ensure!(
            *ports.start() > 0,
            "the ingress port range must not include port 0"
        );
        Ok(Self {
            advertise_host,
            ports,
        })
    }

    /// Parse the flag's `min-max` spelling.
    pub fn parse_ports(s: &str) -> anyhow::Result<RangeInclusive<u16>> {
        let (min, max) = s
            .split_once('-')
            .ok_or_else(|| anyhow!("ingress ports must be `min-max`, not {s:?}"))?;
        let min: u16 = min
            .trim()
            .parse()
            .map_err(|_| anyhow!("ingress port range start is not a port: {s:?}"))?;
        let max: u16 = max
            .trim()
            .parse()
            .map_err(|_| anyhow!("ingress port range end is not a port: {s:?}"))?;
        anyhow::ensure!(min <= max && min > 0, "ingress port range is empty: {s:?}");
        Ok(min..=max)
    }
}

/// The lowest port in the range no existing listener holds — deterministic,
/// so an operator reading `hypeman` listings and a test asserting on ports
/// both know what to expect. `None` is exhaustion, which the caller turns
/// into a create failure naming the knob.
pub(super) fn pick_port(range: &RangeInclusive<u16>, used: &BTreeSet<u16>) -> Option<u16> {
    range.clone().find(|p| !used.contains(p))
}

/// Ports this agent has planned but not yet published — the substrate cannot
/// see them yet (no ingress object exists), so without this two concurrent
/// creates read the same listing and plan the same number.
///
/// Cloned freely (one set behind an `Arc`), and deliberately *only* an
/// in-process view: it is not a distributed lock and does not pretend to be.
/// It settles the same-node race completely, because ports are host-global
/// and a node allocates only for its own host; anything it cannot see is the
/// caller's bounded re-plan to handle.
#[derive(Debug, Clone, Default)]
pub(super) struct PortReservations(Arc<Mutex<BTreeSet<u16>>>);

impl PortReservations {
    /// Take `port` if this process has not already planned it. `None` means
    /// another in-flight create holds it — pick again.
    fn try_take(&self, port: u16) -> Option<PortReservation> {
        let mut held = self.lock();
        held.insert(port)
            .then(|| PortReservation {
                port,
                owner: self.clone(),
            })
    }

    fn snapshot(&self) -> BTreeSet<u16> {
        self.lock().clone()
    }

    /// A poisoned lock must not wedge every future create: the set is a hint,
    /// so the panicking thread's view is taken over rather than propagated.
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeSet<u16>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Holds a planned port until the publish that makes it real (or the failure
/// that abandons it). Released on drop — including the rollback paths — so a
/// create that dies mid-flight never strands a number.
#[derive(Debug)]
pub(super) struct PortReservation {
    port: u16,
    owner: PortReservations,
}

impl PortReservation {
    pub(super) fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for PortReservation {
    fn drop(&mut self) {
        self.owner.lock().remove(&self.port);
    }
}

/// The listener port an existing ingress serves — its one rule's match port,
/// with the contract's default of 80 for an absent one.
///
/// Barista writes exactly one rule per object, so more than one is not ours
/// to reason about and is refused rather than half-read.
pub(super) fn listener_port_of(ingress: &Ingress) -> Result<u16> {
    match ingress.rules.as_slice() {
        [rule] => Ok(rule.r#match.port.unwrap_or(80)),
        other => Err(RuntimeError::Other(anyhow!(
            "ingress '{}' has {} rules where barista wrote one; refusing to guess which \
             listener it serves",
            ingress.name,
            other.len()
        ))),
    }
}

/// Every listener port any ingress on this host holds. Unfiltered on
/// purpose: a listener is host-global, so a port held by an object Barista
/// did not create is still not free.
async fn used_ports(client: &HypemanClient) -> Result<BTreeSet<u16>> {
    Ok(client
        .list_ingresses(None)
        .await
        .map_err(super::runtime::map_client_err)?
        .iter()
        .flat_map(|i| i.rules.iter().map(|r| r.r#match.port.unwrap_or(80)))
        .collect())
}

/// The listener this instance should be published on — decided *before* its
/// sandbox exists, because the number has to ride into the guest as `PORT`.
///
/// Sticky by read-your-own-object: an existing ingress answers with its
/// listener, so retries, cold boots and agent restarts all converge on the
/// same address. Only a missing object picks — the lowest port free in both
/// the substrate's listing *and* this agent's in-flight plans — and the
/// returned [`PortReservation`] holds it until publish, so the create running
/// beside this one picks the next number instead of the same one.
pub(super) async fn planned_port(
    client: &HypemanClient,
    config: &IngressConfig,
    sandbox_name: &str,
    reservations: &PortReservations,
) -> Result<PortReservation> {
    let sticky = match client.get_ingress(sandbox_name).await {
        Ok(existing) => Some(listener_port_of(&existing)?),
        Err(ClientError::Api { status: 404, .. }) => None,
        Err(e) => return Err(super::runtime::map_client_err(e)),
    };
    // Its own listener is already this sandbox's; nothing else may plan it, so
    // a reservation that loses to an in-flight twin means a duplicate create
    // for one name — the caller's re-plan reads the object and converges.
    if let Some(port) = sticky {
        return reservations.try_take(port).ok_or_else(|| {
            RuntimeError::NameConflict(format!(
                "ingress '{sandbox_name}' on port {port}: another create for this sandbox \
                 is publishing it"
            ))
        });
    }
    let used = used_ports(client).await?;
    let mut refused = BTreeSet::new();
    loop {
        let mut taken = used.clone();
        taken.extend(reservations.snapshot());
        taken.extend(&refused);
        let port = pick_port(&config.ports, &taken).ok_or_else(|| {
            RuntimeError::Other(anyhow!(
                "no free ingress port in {}-{}: every listener in the configured range \
                 is taken. Widen BARISTA_INGRESS_PORTS or destroy instances",
                config.ports.start(),
                config.ports.end()
            ))
        })?;
        match reservations.try_take(port) {
            Some(held) => return Ok(held),
            // Lost it to a concurrent plan between the snapshot and the take:
            // remember and pick again rather than hand back a number this
            // agent knows is spoken for.
            None => {
                refused.insert(port);
            }
        }
    }
}

/// Converge the ingress object to `{listener → target}` for a sandbox that
/// now exists. Idempotent: a replay finds the object it wrote and returns; a
/// spec whose `PORT` changed across a cold boot gets its target corrected
/// under the *same* listener, so the published address survives the change.
///
/// A 409 — the planned port was taken between plan and publish — propagates
/// as the conflict it is: the caller fails this create and the retry plans a
/// fresh port. Nothing retries in here, because by this point the guest was
/// already told `PORT` and a different listener would need a different guest.
pub(super) async fn publish(
    client: &HypemanClient,
    config: &IngressConfig,
    node_id: &str,
    instance_id: &InstanceId,
    sandbox_name: &str,
    listener: u16,
    target: u16,
) -> Result<()> {
    match client.get_ingress(sandbox_name).await {
        Ok(existing) => {
            if listener_port_of(&existing)? == listener && existing.rules[0].target.port == target {
                return Ok(());
            }
            // The rule drifted from the plan (a cold boot under a changed
            // spec PORT): keep the address, fix the target.
            client
                .delete_ingress(&existing.id)
                .await
                .map_err(super::runtime::map_client_err)?;
            create(
                client,
                config,
                node_id,
                instance_id,
                sandbox_name,
                listener,
                target,
            )
            .await
        }
        Err(ClientError::Api { status: 404, .. }) => {
            create(
                client,
                config,
                node_id,
                instance_id,
                sandbox_name,
                listener,
                target,
            )
            .await
        }
        Err(e) => Err(super::runtime::map_client_err(e)),
    }
}

async fn create(
    client: &HypemanClient,
    config: &IngressConfig,
    node_id: &str,
    instance_id: &InstanceId,
    sandbox_name: &str,
    listener: u16,
    target: u16,
) -> Result<()> {
    let request = CreateIngressRequest {
        name: sandbox_name.to_string(),
        rules: vec![IngressRule {
            r#match: IngressMatch {
                hostname: config.advertise_host.clone(),
                port: Some(listener),
            },
            target: IngressTarget {
                instance: sandbox_name.to_string(),
                port: target,
            },
        }],
        tags: Some(std::collections::HashMap::from([
            (super::runtime::NODE_TAG.to_string(), node_id.to_string()),
            (
                super::runtime::INSTANCE_TAG.to_string(),
                instance_id.to_string(),
            ),
        ])),
    };
    match client.create_ingress(&request).await {
        Ok(_) => Ok(()),
        // 409 is the substrate arbitrating — name taken or hostname+port in
        // use — and the caller branches on it, so it keeps its own variant.
        Err(ClientError::Api {
            status: 409, body, ..
        }) => Err(RuntimeError::NameConflict(format!(
            "ingress '{sandbox_name}' on port {listener}: {body}"
        ))),
        Err(e) => {
            warn!(instance = %instance_id, error = %e, "could not publish the workload ingress");
            Err(super::runtime::map_client_err(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_port_is_lowest_free_and_exhaustion_is_none() {
        let range = 30000..=30003;
        assert_eq!(pick_port(&range, &BTreeSet::new()), Some(30000));
        assert_eq!(
            pick_port(&range, &BTreeSet::from([30000, 30001])),
            Some(30002),
            "used ports are skipped, deterministically lowest-first"
        );
        // Ports outside the range do not confuse the pick.
        assert_eq!(pick_port(&range, &BTreeSet::from([80, 30000])), Some(30001));
        assert_eq!(
            pick_port(&range, &BTreeSet::from([30000, 30001, 30002, 30003])),
            None,
            "an exhausted range is an answer, not a panic"
        );
    }

    #[test]
    fn a_reservation_holds_a_port_until_it_drops() {
        let held = PortReservations::default();
        let first = held.try_take(30000).expect("free");
        assert!(
            held.try_take(30000).is_none(),
            "a planned port is not free to a create running beside this one"
        );
        assert_eq!(held.snapshot(), BTreeSet::from([30000]));
        drop(first);
        assert!(
            held.try_take(30000).is_some(),
            "the number returns the moment the create that planned it is done"
        );
    }

    #[test]
    fn concurrent_plans_never_pick_the_same_port() {
        // The bug this closes (barista.sh#46): both creates list the same free
        // ports from the substrate — nothing is published yet — so only the
        // in-process reservation can separate them.
        let held = PortReservations::default();
        let range = 30000..=30002;
        let substrate_says_free = BTreeSet::new();

        let plan = |held: &PortReservations| {
            let mut taken = substrate_says_free.clone();
            taken.extend(held.snapshot());
            let port = pick_port(&range, &taken).expect("a free port");
            held.try_take(port).expect("no rival between snapshot and take")
        };

        let a = plan(&held);
        let b = plan(&held);
        let c = plan(&held);
        assert_eq!(
            BTreeSet::from([a.port(), b.port(), c.port()]),
            BTreeSet::from([30000, 30001, 30002]),
            "three concurrent creates get three distinct listeners"
        );
        // Exhaustion is still an honest answer, not a duplicate.
        let mut taken = substrate_says_free.clone();
        taken.extend(held.snapshot());
        assert_eq!(pick_port(&range, &taken), None);
    }

    #[test]
    fn a_poisoned_reservation_lock_does_not_wedge_creates() {
        let held = PortReservations::default();
        let clone = held.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = clone.lock();
            panic!("a create panicked while holding the set");
        }));
        assert!(
            held.try_take(30000).is_some(),
            "one panicking create must not stop every future one from planning"
        );
    }

    #[test]
    fn the_advertise_host_must_be_bare() {
        assert!(IngressConfig::new("88.99.166.242", 30000..=30999).is_ok());
        assert!(IngressConfig::new("node.barista", 30000..=30999).is_ok());
        for bad in [
            "",
            "http://node.barista",
            "node.barista:7777",
            "node.barista/path",
            "node barista",
            "::1",
        ] {
            assert!(
                IngressConfig::new(bad, 30000..=30999).is_err(),
                "{bad:?} must be refused: it is concatenated into host:port"
            );
        }
    }

    #[test]
    fn port_ranges_parse_and_empty_or_zero_ranges_are_refused() {
        assert_eq!(
            IngressConfig::parse_ports("30000-30999").unwrap(),
            30000..=30999
        );
        assert_eq!(
            IngressConfig::parse_ports("8080-8080").unwrap(),
            8080..=8080
        );
        for bad in ["", "30000", "30999-30000", "0-100", "a-b", "1-65536"] {
            assert!(
                IngressConfig::parse_ports(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_multi_rule_ingress_is_refused_not_half_read() {
        let rule = |port: u16| IngressRule {
            r#match: IngressMatch {
                hostname: "h".into(),
                port: Some(port),
            },
            target: IngressTarget {
                instance: "i".into(),
                port,
            },
        };
        let one = Ingress {
            id: "x".into(),
            name: "n".into(),
            rules: vec![rule(30000)],
            tags: Default::default(),
        };
        assert_eq!(listener_port_of(&one).unwrap(), 30000);
        let two = Ingress {
            rules: vec![rule(30000), rule(30001)],
            ..one
        };
        assert!(listener_port_of(&two).is_err());
    }
}
