//! `barista fleet` — the bucket's two object kinds, from a terminal (nap-017 4.1).
//!
//! These verbs deliberately do not touch a node. `apply` writes what should
//! exist and some node picks it up; `resolve` reads where a session runs now.
//! That is the architecture's claim made usable: there is no control plane to
//! ask, so the bucket *is* the API, and a CLI that had to find a healthy node
//! first would be reintroducing the component ADR-002 removed.

use barista_fleet::{Desired, OnOwnerLoss};
use barista_proto::node::v1alpha1 as pb;
// `put`/`get` live on the extension trait in object_store 0.14.
use object_store::ObjectStoreExt;

use crate::FleetCommand;

/// Where the fleet lives. An environment variable rather than a flag, to match
/// the node agent: the bucket is deployment configuration, and its credentials
/// come from the ambient AWS chain either way.
const ENV_BUCKET: &str = "BARISTA_FLEET_BUCKET";

fn store() -> anyhow::Result<std::sync::Arc<dyn object_store::ObjectStore>> {
    let url = std::env::var(ENV_BUCKET).map_err(|_| {
        anyhow::anyhow!(
            "{ENV_BUCKET} is not set. A fleet verb reads and writes the coordination bucket \
             directly — there is no control plane to ask — so it needs to know which bucket. \
             Example: {ENV_BUCKET}=s3://barista?endpoint=http://127.0.0.1:9000"
        )
    })?;
    Ok(barista_fleet::from_url(&url)?)
}

pub(crate) async fn run(what: &FleetCommand, json: bool) -> anyhow::Result<i32> {
    match what {
        FleetCommand::Apply {
            name,
            image,
            digest,
            vcpu,
            mem_mib,
            ttl_seconds,
            on_owner_loss,
            command,
        } => {
            // Built from flags rather than read from a JSON file: `InstanceSpec`
            // is a prost type with no serde derive, so a "just paste the spec"
            // interface would mean hand-writing a second parser for the contract
            // — precisely the duplicate the schema-first rule forbids. The flags
            // mirror `barista create`, which is the shape a caller already knows.
            let spec = pb::InstanceSpec {
                instance_id: ulid::Ulid::generate().to_string(),
                template: Some(pb::TemplateRef {
                    oci: Some(pb::OciImageRef {
                        image: image.clone(),
                        digest: digest.clone(),
                    }),
                    ..Default::default()
                }),
                resources: Some(pb::Resources {
                    vcpu: *vcpu,
                    mem_mib: *mem_mib,
                    disk_mib: 0,
                }),
                process: Some(pb::Process {
                    start_cmd: command.clone(),
                    ..Default::default()
                }),
                ttl_seconds: *ttl_seconds,
                ..Default::default()
            };
            let mut desired = Desired::new(name.clone(), &spec);
            desired.on_owner_loss = match on_owner_loss.as_str() {
                "hold" => OnOwnerLoss::Hold,
                _ => OnOwnerLoss::Coldboot,
            };
            let store = store()?;
            let path = object_store::path::Path::from(format!("desired/{name}"));
            let body = serde_json::to_vec(&desired)?;
            store.put(&path, body.into()).await?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "name": name, "applied": true,
                                        "on_owner_loss": on_owner_loss })
                );
            } else {
                println!("applied {name}");
                println!("  a node will pick it up on its next fleet pass; nothing here chose one");
            }
            Ok(0)
        }

        FleetCommand::Ls => {
            use futures_util::StreamExt;
            let store = store()?;
            let prefix = object_store::path::Path::from("desired");
            let mut listing = store.list(Some(&prefix));
            let mut rows = Vec::new();
            while let Some(meta) = listing.next().await {
                let meta = meta?;
                let name = meta.location.filename().unwrap_or_default().to_string();
                // Owner is a second read per name. Fine at fleet scale and
                // honest about what it costs; a read model is the optimisation
                // ADR-002 §3 said to reach for only when listing hurts.
                let lease = barista_fleet::resolve(&*store, &name).await?;
                rows.push((name, lease));
            }
            if json {
                let value: Vec<_> = rows
                    .iter()
                    .map(|(name, lease)| {
                        serde_json::json!({
                            "name": name,
                            "owner": lease.as_ref().map(|l| l.owner.clone()),
                            "epoch": lease.as_ref().map(|l| l.epoch),
                            "endpoint": lease.as_ref().map(|l| l.endpoint.clone()),
                            "instance_id": lease.as_ref().map(|l| l.instance_id.clone()),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string(&value)?);
                return Ok(0);
            }
            if rows.is_empty() {
                println!("no sessions desired");
                return Ok(0);
            }
            println!("{:<28} {:<24} {:<7} ENDPOINT", "NAME", "OWNER", "EPOCH");
            for (name, lease) in rows {
                match lease {
                    Some(l) => println!(
                        "{:<28} {:<24} {:<7} {}",
                        name,
                        l.owner,
                        l.epoch,
                        if l.endpoint.is_empty() {
                            "-"
                        } else {
                            &l.endpoint
                        }
                    ),
                    // Desired but unowned is a real and temporary state — no node
                    // has taken it yet — and saying so beats printing a blank.
                    None => println!("{:<28} {:<24} {:<7} -", name, "(unowned)", "-"),
                }
            }
            Ok(0)
        }

        FleetCommand::Resolve { name } => {
            let store = store()?;
            let lease = barista_fleet::resolve(&*store, name).await?;
            match lease {
                Some(l) if json => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "name": name, "owner": l.owner, "epoch": l.epoch,
                            "endpoint": l.endpoint, "instance_id": l.instance_id,
                            "expires_ms": l.expires_ms,
                        })
                    );
                    Ok(0)
                }
                Some(l) => {
                    println!("{name} is owned by {} at epoch {}", l.owner, l.epoch);
                    if !l.endpoint.is_empty() {
                        println!("  reach it at {}", l.endpoint);
                    }
                    Ok(0)
                }
                // Exit 1, not an error: "nobody owns this" is an answer, and a
                // script asking "where does this run" wants to branch on it
                // rather than parse a message.
                None if json => {
                    println!("{}", serde_json::json!({ "name": name, "owner": null }));
                    Ok(1)
                }
                None => {
                    println!("{name} is not owned by any node");
                    Ok(1)
                }
            }
        }
    }
}
