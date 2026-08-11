//! Preflight against a real `hypeman-api`.
//!
//! Self-skips when the substrate is absent, the same way the Docker-backed tests
//! do — a developer without hypeman installed should still get a green
//! `make check`, and CI gains the substrate with the Linux job (task 5.5).

mod common;

use barista_node_agent::runtime::hypeman::{config::Config, preflight};

/// Reachable *and* authorized. `/health` is the substrate's only unauthenticated
/// operation, so a host that answers it may still reject every real call — which is
/// exactly what preflight now checks for, and what this test would otherwise trip
/// over rather than verify.
async fn provisioned(config: &Config) -> bool {
    config.client().health().await.is_ok() && config.client().list_instances(None).await.is_ok()
}

/// The point of the preflight: on a correctly provisioned host it says nothing.
#[tokio::test]
async fn a_provisioned_host_reports_no_problems() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token configured");
        return;
    };
    if !provisioned(&config).await {
        eprintln!(
            "SKIP: hypeman-api not reachable or not authorized at {}",
            config.base_url
        );
        return;
    }
    let report = preflight::run(&config).await;
    assert!(
        report.is_empty(),
        "preflight found problems on a host where hypeman is running:\n{}",
        preflight::describe(&report.all())
    );
}

/// The failure path is the one that earns its keep, so assert it names the thing
/// and the fix rather than just failing.
#[tokio::test]
async fn an_unreachable_substrate_is_named_with_a_remedy() {
    let report = preflight::run(&Config::new("http://127.0.0.1:1", None)).await;
    let reachability = report
        .problems
        .iter()
        .find(|p| p.what.contains("not reachable"))
        .expect("an unreachable substrate must be reported");
    assert!(reachability.remedy.contains("BARISTA_HYPEMAN_URL"));
    assert!(
        reachability.why_it_matters.contains("already running"),
        "must state that live sessions are unaffected: {}",
        reachability.why_it_matters
    );
}

/// The check that matters most and looks like paranoia: an API which answers an
/// **anonymous** caller leaks every guest's token, because the token rides in the
/// sandbox environment and `GET /instances` returns that environment verbatim.
///
/// Asserted against the real substrate, since the whole point is a property of the
/// deployment rather than of our code. On a correctly configured host it finds
/// nothing — which is exactly what makes it worth having.
#[tokio::test]
async fn an_api_that_answers_anonymously_is_reported_as_a_credential_leak() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token configured");
        return;
    };
    if !provisioned(&config).await {
        eprintln!("SKIP: hypeman-api not reachable or not authorized");
        return;
    }

    // Ask the substrate directly, with no credential at all.
    let anonymous =
        barista_node_agent::runtime::hypeman::config::Config::new(&config.base_url, None);
    let answers_anonymously = anonymous.client().list_instances(None).await.is_ok();

    let report = preflight::run(&config).await;
    let flagged = report.open_substrate.is_some();

    assert_eq!(
        flagged, answers_anonymously,
        "preflight must flag an anonymously-readable API exactly when there is one; \
         substrate answered anonymously: {answers_anonymously}, preflight flagged: {flagged}"
    );
}

/// The positive branch, deterministically — the real substrate on a correctly
/// configured host answers 401, so the check that *fires* would otherwise never be
/// exercised. A three-line server standing in for a wide-open `hypeman-api` is
/// enough, because the only thing under test is what preflight concludes from a
/// 200 to an anonymous caller.
#[tokio::test]
async fn a_wide_open_api_is_named_with_the_consequence_and_the_fix() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        // Answer everything, unauthenticated, the way a tokenless daemon would.
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                          Content-Length: 2\r\n\r\n[]",
                    )
                    .await;
            });
        }
    });

    let config = Config::new(format!("http://{addr}"), Some("we-hold-a-token".into()));
    let report = preflight::run(&config).await;

    // The finding rides apart from ordinary problems because the caller must
    // treat it differently: `main` refuses to boot onto it (finding M1), while
    // every other problem is a warning.
    let leak = report.open_substrate.as_ref().unwrap_or_else(|| {
        panic!(
            "an anonymously-readable API must be reported as an open substrate:\n{}",
            preflight::describe(&report.all())
        )
    });
    // Holding a token of our own must not excuse it: the door being open is the
    // finding, not whether we happen to knock politely.
    assert!(
        leak.why_it_matters.contains("arbitrary code"),
        "the consequence must be spelled out, not implied: {}",
        leak.why_it_matters
    );
    assert!(
        leak.remedy.contains("bearer token"),
        "and it must say how to close it: {}",
        leak.remedy
    );
}
