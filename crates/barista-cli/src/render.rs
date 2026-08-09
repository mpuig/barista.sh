//! Turning Contract A's types into something to read, or something to parse.
//!
//! Two audiences, one rule: `--json` emits the proto's own field names so a
//! script is reading the contract rather than a CLI-shaped view of it. The human
//! form is free to be lossy; the JSON form is not.

use barista_proto::node::v1alpha1 as pb;
use tokio_stream::StreamExt;

/// Enum names without their prefix — `RUNNING`, not `INSTANCE_STATE_RUNNING`.
///
/// Prefixes are stripped by name rather than by splitting on the last underscore,
/// which is what this did first: that turned `ERROR_REASON_CAPABILITY_MISSING`
/// into "MISSING", which reads as though something were absent rather than as the
/// name of a refusal. Multi-word suffixes are the common case, not the exception.
///
/// Only for human output. The JSON keeps the full name, because that is what the
/// contract calls it and a script should not have to reverse this.
fn short(name: &str) -> &str {
    const PREFIXES: &[&str] = &[
        "ERROR_REASON_",
        "INSTANCE_STATE_",
        "OPERATION_STATE_",
        "SNAPSHOT_KIND_",
        "SNAPSHOT_TIER_",
        "SUBSTRATE_HEALTH_",
        "EVENT_TYPE_",
        "TTL_ACTION_",
    ];
    PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
}

fn state_name(state: i32) -> &'static str {
    pb::InstanceState::try_from(state)
        .unwrap_or_default()
        .as_str_name()
}

pub(crate) fn error(e: &anyhow::Error, json: bool) {
    if json {
        let value = serde_json::json!({ "error": e.to_string() });
        eprintln!("{value}");
    } else {
        eprintln!("barista: {e}");
    }
}

/// A refusal from the node, in the terms the node used.
///
/// `tonic::Status`'s own `Display` includes the metadata map and header noise,
/// which buries the one sentence a reader needs. This prints the reason and the
/// message the node actually wrote.
pub(crate) fn status(status: &tonic::Status, json: bool) {
    let reason = crate::follow::reason_of(status);
    // The node prefixes its message with the reason name; saying it twice reads
    // badly, so strip it and let the rendering put it where it belongs.
    let message = status
        .message()
        .strip_prefix(reason.as_str_name())
        .map(|rest| rest.trim_start_matches([':', ' ']))
        .unwrap_or_else(|| status.message());
    if json {
        eprintln!(
            "{}",
            serde_json::json!({
                "error": message,
                "reason": reason.as_str_name(),
                "code": status.code().to_string(),
            })
        );
    } else {
        eprintln!("barista: {} — {}", short(reason.as_str_name()), message);
    }
}

/// What became of an operation the CLI just followed.
///
/// The failure line carries the machine-readable reason as well as the message,
/// because the reason is what a human is going to search for and what the exit
/// code was derived from — printing only prose would hide the one field that
/// distinguishes "will never work" from "try again".
pub(crate) fn outcome(outcome: &crate::follow::Outcome, instance_id: &str, json: bool) {
    let reason = outcome.reason();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "instance_id": instance_id,
                "op_id": outcome.op.op_id,
                "kind": outcome.op.kind,
                "state": pb::OperationState::try_from(outcome.op.state)
                    .unwrap_or_default().as_str_name(),
                "reason": (!outcome.succeeded()).then(|| reason.as_str_name()),
                "message": outcome.message(),
                // Degradations travel here too: an operation can succeed and still
                // have lost something (a cold-boot fallback, a pause without
                // memory), and a script needs that without parsing the event log.
                "degraded": (!outcome.op.degraded.is_empty()).then(|| outcome.op.degraded.clone()),
                // Not a degradation and deliberately a separate field: nothing was
                // downgraded, the workload was simply stopped while it was copied
                // (nap-015). A consumer holding an open session branches on this.
                "froze_workload": outcome.op.froze_workload,
            })
        );
        return;
    }
    if outcome.succeeded() {
        println!("{} {}", outcome.op.kind, instance_id);
        if outcome.op.froze_workload {
            println!("  the workload was stopped while it was copied, and is running again");
        }
        if !outcome.op.degraded.is_empty() {
            println!("  degraded: {}", outcome.op.degraded);
        }
    } else {
        eprintln!(
            "{} {} failed: {} — {}",
            outcome.op.kind,
            instance_id,
            short(reason.as_str_name()),
            outcome.message()
        );
    }
}

pub(crate) fn node_info(info: &pb::NodeInfo, json: bool) {
    if json {
        println!("{}", serde_json::to_string(&node_info_value(info)).unwrap());
        return;
    }
    println!("node       {}", info.node_id);
    println!("arch       {}", info.arch);
    println!("cpu class  {}", info.cpu_class);
    println!("agent      {}", info.agent_version);
    for runtime in &info.runtimes {
        let health = pb::SubstrateHealth::try_from(runtime.health).unwrap_or_default();
        println!("\nruntime    {} {}", runtime.name, runtime.version);
        // Health is printed even when it is fine: an operator reading this wants
        // to know the question was asked, not to infer it from silence.
        print!("substrate  {}", short(health.as_str_name()));
        if !runtime.health_detail.is_empty() {
            print!(" — {}", runtime.health_detail);
        }
        println!();
        if let Some(caps) = &runtime.capabilities {
            // Only what it *can* do, so the list reads as a promise rather than a
            // form with most boxes unticked.
            let mut yes = Vec::new();
            for (label, on) in [
                ("memory-snapshot", caps.memory_snapshot),
                ("disk-snapshot", caps.disk_snapshot),
                ("live-checkpoint", caps.live_checkpoint),
                ("guest-agent", caps.guest_agent),
                ("hardware-isolation", caps.hardware_isolation),
                ("lazy-restore", caps.lazy_restore),
                ("cow-fork", caps.cow_fork),
                ("egress-control", caps.egress_control),
            ] {
                if on {
                    yes.push(label);
                }
            }
            println!(
                "can        {}",
                if yes.is_empty() {
                    "nothing beyond the basics".to_string()
                } else {
                    yes.join(", ")
                }
            );
        }
    }
}

fn node_info_value(info: &pb::NodeInfo) -> serde_json::Value {
    serde_json::json!({
        "node_id": info.node_id,
        "arch": info.arch,
        "cpu_class": info.cpu_class,
        "agent_version": info.agent_version,
        "runtimes": info.runtimes.iter().map(|r| serde_json::json!({
            "name": r.name,
            "version": r.version,
            "health": pb::SubstrateHealth::try_from(r.health).unwrap_or_default().as_str_name(),
            "health_detail": r.health_detail,
            "capabilities": r.capabilities.as_ref().map(|c| serde_json::json!({
                "memory_snapshot": c.memory_snapshot,
                "disk_snapshot": c.disk_snapshot,
                "live_checkpoint": c.live_checkpoint,
                "guest_agent": c.guest_agent,
                "hardware_isolation": c.hardware_isolation,
                "lazy_restore": c.lazy_restore,
                "cow_fork": c.cow_fork,
                "egress_control": c.egress_control,
            })),
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn instances(instances: &[pb::Instance], json: bool) {
    if json {
        let value: Vec<_> = instances.iter().map(instance_value).collect();
        println!("{}", serde_json::to_string(&value).unwrap());
        return;
    }
    if instances.is_empty() {
        println!("no instances");
        return;
    }
    println!(
        "{:<28} {:<14} {:<7} {:<8} {:<24} SNAPSHOT",
        "INSTANCE", "STATE", "READY", "WAKE", "EGRESS"
    );
    let now = std::time::SystemTime::now();
    for instance in instances {
        let id = instance
            .spec
            .as_ref()
            .map(|s| s.instance_id.as_str())
            .unwrap_or("?");
        println!(
            "{:<28} {:<14} {:<7} {:<8} {:<24} {}",
            id,
            state_cell(instance),
            if instance.ready { "yes" } else { "no" },
            instance
                .wake_at
                .as_ref()
                .map(|at| crate::wake::until(at.seconds, now))
                .unwrap_or_else(|| "-".to_string()),
            egress_label(instance.spec.as_ref().and_then(|s| s.egress)),
            if instance.latest_snapshot_id.is_empty() {
                "-"
            } else {
                &instance.latest_snapshot_id
            }
        );
    }
}

/// The STATE cell, carrying the exit code when there is one — `STOPPED(3)`.
///
/// Folded into the state rather than given a column of its own, because it only
/// ever qualifies one state and an empty sixth column on every running instance
/// would cost more than it explains. A stop with no exit code stays plain
/// `STOPPED`: absent is absent, and "STOPPED(0)" would be a claim nobody made
/// (nap-013 design decision 5). `--json` carries the whole structure.
fn state_cell(instance: &pb::Instance) -> String {
    let state = short(state_name(instance.state)).to_string();
    match instance.stop_reason.as_ref().and_then(|r| r.exit_code) {
        Some(code) => format!("{state}({code})"),
        None => state,
    }
}

/// One instance's egress policy, short enough for a column (nap-014 task 3.1).
///
/// A column rather than a `barista get`-only detail because the question it answers —
/// which of these sandboxes can reach the internet — is a question about the
/// whole list. `-` covers both "no policy" and `mediated: false`, since neither
/// confines anything and pretending they differ would suggest one of them does.
fn egress_label(policy: Option<pb::EgressPolicy>) -> String {
    match policy {
        Some(p) if p.mediated => match p.mode() {
            pb::EgressMode::HttpHttpsOnly => "mediated:http-https-only".to_string(),
            _ => "mediated".to_string(),
        },
        _ => "-".to_string(),
    }
}

fn instance_value(instance: &pb::Instance) -> serde_json::Value {
    serde_json::json!({
        "instance_id": instance.spec.as_ref().map(|s| s.instance_id.clone()).unwrap_or_default(),
        "state": state_name(instance.state),
        "ready": instance.ready,
        "runtime": instance.runtime,
        "latest_snapshot_id": instance.latest_snapshot_id,
        "ttl_deadline": instance.ttl_deadline.as_ref().map(|t| t.seconds),
        "wake_at": instance.wake_at.as_ref().map(|t| t.seconds),
        "stop_reason": instance.stop_reason.as_ref().map(stop_reason_value),
        // Structured rather than the human column's label: the JSON form is a
        // contract surface, so it carries the proto's own field names and the
        // enum's full name. `null` is the absent policy.
        "egress": instance.spec.as_ref().and_then(|s| s.egress).map(|e| serde_json::json!({
            "mediated": e.mediated,
            "mode": e.mode().as_str_name(),
        })),
    })
}

/// A stop reason as JSON, with `exit_code` absent rather than zero when the
/// substrate reported none — the distinction the whole field exists for, and one
/// a script would otherwise have to guess at.
fn stop_reason_value(reason: &pb::StopReason) -> serde_json::Value {
    serde_json::json!({
        "requested": reason.requested,
        "exit_code": reason.exit_code,
        "detail": reason.detail,
    })
}

pub(crate) fn snapshots(snapshots: &[pb::Snapshot], json: bool) {
    if json {
        let value: Vec<_> = snapshots
            .iter()
            .map(|s| {
                serde_json::json!({
                    "snapshot_id": s.snapshot_id,
                    "instance_id": s.instance_id,
                    // Empty for the snapshot a pause leaves behind; identity is
                    // always the id, so this is a label and never a key.
                    "name": s.name,
                    "kind": pb::SnapshotKind::try_from(s.kind).unwrap_or_default().as_str_name(),
                    "tier": pb::SnapshotTier::try_from(s.tier).unwrap_or_default().as_str_name(),
                    "cpu_class": s.cpu_class,
                    "template_hash": s.template_hash,
                    "runtime_bundle_ref": s.runtime_bundle_ref,
                    "size_bytes": s.size_bytes,
                    // Distinguishes "no hook configured" from "could not ask the
                    // guest", which decides whether the capture is known-clean.
                    "pre_snapshot_hook": s.pre_snapshot_hook.map(|h| serde_json::json!({
                        "ran": h.ran, "timed_out": h.timed_out, "exit_code": h.exit_code,
                    })),
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&value).unwrap());
        return;
    }
    if snapshots.is_empty() {
        println!("no snapshots");
        return;
    }
    println!(
        "{:<28} {:<20} {:<28} {:<16} QUIESCED",
        "SNAPSHOT", "NAME", "INSTANCE", "KIND"
    );
    for s in snapshots {
        let kind = pb::SnapshotKind::try_from(s.kind).unwrap_or_default();
        // Three states, not two: a hook that could not be asked is not the same
        // as one that was asked and had nothing to do.
        let quiesced = match s.pre_snapshot_hook {
            None => "unknown",
            Some(h) if h.timed_out => "timed out",
            Some(h) if h.ran && h.exit_code == 0 => "yes",
            Some(h) if h.ran => "failed",
            Some(_) => "no hook",
        };
        println!(
            "{:<28} {:<20} {:<28} {:<16} {}",
            s.snapshot_id,
            if s.name.is_empty() { "-" } else { &s.name },
            s.instance_id,
            short(kind.as_str_name()),
            quiesced
        );
    }
}

pub(crate) async fn events(
    mut stream: tonic::Streaming<pb::Event>,
    json: bool,
) -> anyhow::Result<()> {
    while let Some(event) = stream.next().await {
        let event = event?;
        let kind = pb::EventType::try_from(event.r#type).unwrap_or_default();
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "cursor": event.cursor,
                    "type": kind.as_str_name(),
                    "instance_id": event.instance_id,
                    "op_id": event.op_id,
                    "state": state_name(event.state),
                    "message": event.message,
                    "stop_reason": event.stop_reason.as_ref().map(stop_reason_value),
                })
            );
        } else {
            let mut detail = if event.message.is_empty() {
                short(state_name(event.state)).to_string()
            } else {
                event.message.clone()
            };
            // The stop reason rides the state change, so a human following the
            // stream sees how the workload ended without a second lookup.
            if let Some(reason) = &event.stop_reason {
                if let Some(code) = reason.exit_code {
                    detail.push_str(&format!(" (workload exited {code})"));
                }
                if !reason.detail.is_empty() {
                    detail.push_str(&format!(" ({})", reason.detail));
                }
            }
            println!(
                "{:<8} {:<20} {:<28} {}",
                event.cursor,
                short(kind.as_str_name()),
                event.instance_id,
                detail
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_output_drops_the_enum_prefix_but_json_keeps_it() {
        assert_eq!(short("INSTANCE_STATE_RUNNING"), "RUNNING");
        // The case that caught this: splitting on the last underscore rendered
        // this as "MISSING", which names the wrong thing entirely.
        assert_eq!(
            short("ERROR_REASON_CAPABILITY_MISSING"),
            "CAPABILITY_MISSING"
        );
        assert_eq!(short("SNAPSHOT_KIND_MEMORY_AND_DISK"), "MEMORY_AND_DISK");
        // An unrecognised name is left alone rather than mangled.
        assert_eq!(short("RUNNING"), "RUNNING");
    }

    /// The JSON form is a contract surface: a script reading it must see the
    /// proto's own spelling, not a prettified one.
    #[test]
    fn json_uses_the_contracts_own_names() {
        let info = pb::NodeInfo {
            node_id: "n".into(),
            runtimes: vec![pb::RuntimeInfo {
                name: "fake".into(),
                health: pb::SubstrateHealth::Healthy as i32,
                ..Default::default()
            }],
            ..Default::default()
        };
        let value = node_info_value(&info);
        assert_eq!(value["runtimes"][0]["health"], "SUBSTRATE_HEALTH_HEALTHY");
    }
}
