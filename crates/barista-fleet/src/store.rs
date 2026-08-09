//! Building the object store from a URL plus the ambient credential chain.
//!
//! Here rather than in the node agent because both consumers need it and
//! neither should own it: the agent joins a fleet, the CLI reads and writes the
//! same two object kinds without a node in the path, and a gateway will resolve
//! against it later. A CLI that had to link the whole node agent to reach a
//! bucket would be the drift design decision 1 exists to prevent.

use std::sync::Arc;

use object_store::aws::AmazonS3Builder;
use object_store::ObjectStore;

use crate::{Error, Result};

/// Accepts `s3://<bucket>?endpoint=<url>`, `s3://<host>/<bucket>`, and
/// `https://<host>/<bucket>` — the last two because they are the forms
/// `object_store` documents for R2 and the forms a person writes.
///
/// Credentials come from the environment (`AWS_ACCESS_KEY_ID` and friends, or an
/// instance role), never from a flag: a bucket credential is deployment
/// configuration, and a flag puts it in a process list and a shell history.
///
/// # The unverified backend warning
///
/// MinIO and Cloudflare R2 have been *measured* to honour the conditional writes
/// this protocol needs (ADR-002 §3.1). AWS S3 and Azure Blob document the same
/// primitives and are expected to; nothing here has observed them. A backend
/// whose conditional write is not atomic does not fail — it silently lets two
/// nodes own one session — so a node points at an unmeasured endpoint with a
/// warning rather than in silence.
pub fn from_url(url: &str) -> Result<Arc<dyn ObjectStore>> {
    let parts = parse(url).ok_or_else(|| {
        Error::Config(format!(
            "fleet bucket URL '{url}' is not understood. Accepted: \
             s3://<bucket>?endpoint=<url>, s3://<host>/<bucket>, or \
             https://<host>/<bucket>"
        ))
    })?;
    let mut builder = AmazonS3Builder::from_env()
        .with_bucket_name(parts.bucket)
        // The primitive ADR-002 measured. Without it every conditional write is
        // answered `NotImplemented` and the protocol does not exist — loudly,
        // which is the right way for it to be absent.
        .with_conditional_put(object_store::aws::S3ConditionalPut::ETagMatch);
    if let Some(endpoint) = parts.endpoint {
        // `s3://host/bucket` yields a bare host; `with_endpoint` wants a URL.
        // https, because a plaintext endpoint has to be written as one — see
        // below.
        let endpoint = if endpoint.contains("://") {
            endpoint.to_string()
        } else {
            format!("https://{endpoint}")
        };
        let plaintext = endpoint.starts_with("http://");
        builder = builder.with_endpoint(&endpoint);
        if plaintext {
            // A plaintext endpoint is a local MinIO or a test. Allowed only
            // because the URL said `http://` explicitly, so a production URL
            // cannot become plaintext through a typo elsewhere.
            builder = builder.with_allow_http(true);
        }
    }
    warn_if_unverified(parts.endpoint);
    Ok(Arc::new(builder.build()?))
}

struct Parts<'a> {
    bucket: &'a str,
    endpoint: Option<&'a str>,
}

/// `s3://<bucket>` with an optional `?endpoint=`, or the vendor URL forms a
/// person actually writes: `s3://<host>/<bucket>` and `https://<host>/<bucket>`.
///
/// The second shape is here because it is what `object_store` itself documents
/// for R2 and what the first real user typed. A parser that accepted only the
/// query form would have been technically defensible and practically a trap.
fn parse(url: &str) -> Option<Parts<'_>> {
    // `https://host/bucket` — no scheme rewriting, the host is the endpoint.
    if let Some(rest) = url.strip_prefix("https://") {
        let (host, bucket) = rest.split_once('/')?;
        return (!host.is_empty() && !bucket.is_empty()).then_some(Parts {
            bucket: bucket.trim_end_matches('/'),
            endpoint: Some(&url[..url.len() - bucket.len() - 1]),
        });
    }

    let rest = url.strip_prefix("s3://")?;
    let (head, query) = match rest.split_once('?') {
        Some((head, query)) => (head, Some(query)),
        None => (rest, None),
    };
    if head.is_empty() {
        return None;
    }

    // `s3://host/bucket`: a slash means the first segment is an endpoint host,
    // not a bucket. Buckets cannot contain slashes, so this is unambiguous.
    if let Some((host, bucket)) = head.split_once('/') {
        let bucket = bucket.trim_end_matches('/');
        if bucket.is_empty() {
            return None;
        }
        // Scheme assumed https: an `s3://` URL naming a remote host is a cloud
        // endpoint, and a plaintext one is written `?endpoint=http://…`.
        return Some(Parts {
            bucket,
            endpoint: Some(host),
        });
    }

    let endpoint = query.and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("endpoint="))
            .filter(|e| !e.is_empty())
    });
    Some(Parts {
        bucket: head,
        endpoint,
    })
}

/// Say so, once per node start, when the bucket is a backend nobody measured.
///
/// Deliberately a log line and not a refusal: refusing would make Barista
/// unusable on the backends it is *designed* for, on the strength of a test not
/// having been run. The operator's real need is to know which of the two
/// situations they are in.
///
/// The measured set is MinIO and Cloudflare R2 (ADR-002 §3.1, the R2 row closed
/// 2026-08-08). AWS S3 and Azure Blob document the same primitives and are
/// expected to work; nothing here has observed them.
fn warn_if_unverified(endpoint: Option<&str>) {
    let measured = endpoint.is_some_and(|e| {
        e.contains("127.0.0.1") || e.contains("localhost") || e.contains("r2.cloudflarestorage.com")
    });
    if !measured {
        tracing::warn!(
            "fleet coordination is running against a backend whose conditional writes this \
             project has not measured (ADR-002 §3.1 has rows for MinIO and Cloudflare R2). The \
             primitives are standard and S3 and Azure document them, but a backend that \
             implements them non-atomically does not error — it lets two nodes own one session. \
             Close the gap by running `cargo test -p barista-fleet --test fencing` against this \
             bucket and recording the row."
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_url_yields_a_bucket_and_an_optional_endpoint() {
        let p = super::parse("s3://barista").unwrap();
        assert_eq!(p.bucket, "barista");
        assert_eq!(p.endpoint, None);

        let p = super::parse("s3://barista?endpoint=http://127.0.0.1:9000").unwrap();
        assert_eq!(p.bucket, "barista");
        assert_eq!(p.endpoint, Some("http://127.0.0.1:9000"));
    }

    /// The vendor forms, which are what a person actually types. The R2 URL
    /// here is the one the first real user pasted, and the query-only parser
    /// would have read the whole host-and-path as a bucket name.
    #[test]
    fn the_vendor_url_forms_parse_too() {
        let p = super::parse("s3://3ee0.r2.cloudflarestorage.com/barista").unwrap();
        assert_eq!(p.bucket, "barista");
        assert_eq!(p.endpoint, Some("3ee0.r2.cloudflarestorage.com"));

        let p = super::parse("https://3ee0.r2.cloudflarestorage.com/barista").unwrap();
        assert_eq!(p.bucket, "barista");
        assert_eq!(
            p.endpoint,
            Some("https://3ee0.r2.cloudflarestorage.com"),
            "the https form keeps its scheme; the bare-host form gets one added at build time"
        );

        // A trailing slash is a typo, not a different bucket.
        assert_eq!(
            super::parse("s3://host.example/barista/").unwrap().bucket,
            "barista"
        );
    }

    /// Refused rather than half-understood: a URL nobody parsed would otherwise
    /// become a node running alone while its operator believes it joined a fleet.
    #[test]
    fn an_unusable_url_is_refused() {
        for url in [
            "https://barista",
            "s3://",
            "barista",
            "",
            "s3://host.example/",
            "https://host.example/",
        ] {
            assert!(super::parse(url).is_none(), "{url} should not parse");
        }
    }
}
