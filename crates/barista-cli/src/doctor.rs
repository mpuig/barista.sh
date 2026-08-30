//! `barista doctor` — is this node able to do its job?
//!
//! The task list wrote this as "runsc version, overlayfs, data dir, agent
//! reachability", from before ADR-001 v2 made `hypeman` the rank-1 substrate and
//! `runsc` a deferred rank-2 tier. Checking for a runsc that Phase 1 does not use
//! would be theatre. What replaced it is the same *question* asked of what the
//! node actually runs: can I reach it, what does it say its substrate is doing,
//! and can it reach a guest.
//!
//! Everything here is asked over Contract A rather than probed locally, because
//! `barista` may be talking to a node on another host — a doctor that inspected the
//! *caller's* filesystem would confidently describe the wrong machine.

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use tonic::transport::Channel;

pub(crate) struct Finding {
    pub ok: bool,
    pub what: String,
    pub detail: String,
}

/// Run the checks. Returns them in the order they were asked, so the output
/// reads as a narrowing sequence rather than an unordered pile.
pub(crate) async fn run(client: &mut NodeAgentClient<Channel>, address: &str) -> Vec<Finding> {
    let mut findings = vec![Finding {
        ok: true,
        what: "node reachable".into(),
        // Getting here at all means the connection succeeded, since `connect`
        // happens before any subcommand runs.
        detail: address.to_string(),
    }];

    let info = match client.get_node_info(pb::GetNodeInfoRequest {}).await {
        Ok(response) => response.into_inner(),
        Err(e) => {
            findings.push(Finding {
                ok: false,
                what: "node answers GetNodeInfo".into(),
                detail: format!("{e}; the node is listening but not serving Contract A"),
            });
            return findings;
        }
    };

    findings.push(Finding {
        ok: true,
        what: "node identity".into(),
        detail: format!("{} ({}, {})", info.node_id, info.arch, info.cpu_class),
    });

    if info.runtimes.is_empty() {
        findings.push(Finding {
            ok: false,
            what: "a runtime is registered".into(),
            detail: "the node reports no runtimes, so it can run nothing".into(),
        });
        return findings;
    }

    for runtime in &info.runtimes {
        let health = pb::SubstrateHealth::try_from(runtime.health).unwrap_or_default();
        findings.push(Finding {
            // UNSPECIFIED is not a failure: it means the agent is older than the
            // field. Reporting it as broken would make an upgrade look like an
            // outage.
            ok: health != pb::SubstrateHealth::Unreachable,
            what: format!("substrate for '{}'", runtime.name),
            detail: match health {
                pb::SubstrateHealth::Healthy => format!("healthy ({})", runtime.version),
                pb::SubstrateHealth::Unreachable => format!(
                    "unreachable — {}. Instances already running are unaffected, but \
                     nothing can be created, started or destroyed until it returns",
                    runtime.health_detail
                ),
                _ => "not reported by this agent".into(),
            },
        });

        let caps = runtime.capabilities.unwrap_or_default();
        // The guest agent is the difference between a node that can run a
        // workload and one that can also be *worked in*: no exec, no file
        // access, no readiness.
        findings.push(Finding {
            ok: caps.guest_agent,
            what: format!("guest channel for '{}'", runtime.name),
            detail: if caps.guest_agent {
                "available: exec, file access and readiness probes will work".into()
            } else {
                "absent: `barista exec`, `barista cp` and readiness will all fail. The node was \
                 most likely started without --guest-bin"
                    .into()
            },
        });
        findings.push(pause_finding(&runtime.name, caps.memory_snapshot));
    }

    // Reachable and answering is not the same as working: an instance the node
    // cannot list means the journal is unreadable.
    match crate::instances::list_all(client).await {
        Ok(instances) => findings.push(Finding {
            ok: true,
            what: "journal readable".into(),
            detail: format!("{} instance(s)", instances.len()),
        }),
        Err(e) => findings.push(Finding {
            ok: false,
            what: "journal readable".into(),
            detail: format!("{e}; the node cannot list its own instances"),
        }),
    }

    findings
}

/// The memory-continuity check behind the strict deployment gate.
///
/// Kept separate from rendering so a disk-only runtime cannot accidentally
/// become a healthy result because its explanation sounds informative.
fn pause_finding(runtime: &str, memory_snapshot: bool) -> Finding {
    Finding {
        ok: memory_snapshot,
        what: format!("pause/resume for '{runtime}'"),
        detail: if memory_snapshot {
            "memory snapshots available: pause keeps the session".into()
        } else {
            "disk-only: memory snapshots are unavailable. Select a memory-capable \
             runtime such as hypeman on a supported host; use `barista node info` \
             when you only need capability inventory"
                .into()
        },
    }
}

/// Print findings, and return the process exit code.
pub(crate) fn report(findings: &[Finding], json: bool) -> i32 {
    let failed = findings.iter().filter(|f| !f.ok).count();
    if json {
        let value: Vec<_> = findings
            .iter()
            .map(|f| serde_json::json!({ "ok": f.ok, "check": f.what, "detail": f.detail }))
            .collect();
        println!("{}", serde_json::to_string(&value).unwrap());
    } else {
        for finding in findings {
            println!(
                "{} {:<28} {}",
                if finding.ok { "ok  " } else { "FAIL" },
                finding.what,
                finding.detail
            );
        }
        if failed > 0 {
            println!("\n{failed} check(s) failed");
        }
    }
    // Non-zero on any failure, so `barista doctor` is usable as a readiness gate in
    // a script rather than only as something to read.
    if failed > 0 {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_only_is_a_failed_readiness_check() {
        let finding = pause_finding("fake", false);
        assert!(!finding.ok);
        assert!(finding.detail.contains("disk-only"));
        assert!(finding.detail.contains("memory-capable"));
        assert_eq!(report(&[finding], true), 1);
    }

    #[test]
    fn memory_pause_passes_the_readiness_check() {
        let finding = pause_finding("hypeman", true);
        assert!(finding.ok);
        assert!(finding.detail.contains("pause keeps the session"));
        assert_eq!(report(&[finding], true), 0);
    }
}
