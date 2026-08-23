//! Contract round-trip stub server (nap-001, task 3.3).
//!
//! Serves `GetNodeInfo` with fixed, assertable data; every other RPC returns
//! `UNIMPLEMENTED`. Used by the Python round-trip test
//! (`py/barista-proto/tests/test_roundtrip.py`) to prove the generated clients and
//! servers speak the same contract.
//!
//! Usage: `stub_server <port>` — prints `LISTENING <port>` once bound.

// tonic::Status is large by design; this is the standard allowance for tonic services.
#![allow(clippy::result_large_err)]

use std::pin::Pin;

use barista_proto::node::v1alpha1::node_agent_server::{NodeAgent, NodeAgentServer};
use barista_proto::node::v1alpha1::*;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, Streaming};

type Rsp<T> = Result<Response<T>, Status>;
type Stream<T> = Pin<Box<dyn tokio_stream::Stream<Item = Result<T, Status>> + Send>>;

#[derive(Default)]
struct Stub;

fn nyi<T>() -> Rsp<T> {
    Err(Status::unimplemented("stub server: GetNodeInfo only"))
}

#[tonic::async_trait]
impl NodeAgent for Stub {
    async fn get_node_info(&self, _r: Request<GetNodeInfoRequest>) -> Rsp<NodeInfo> {
        Ok(Response::new(NodeInfo {
            node_id: "01STUBNODE0000000000000000".to_string(),
            arch: "aarch64".to_string(),
            cpu_class: "stub-cpu-class".to_string(),
            runtimes: vec![RuntimeInfo {
                name: "fake".to_string(),
                capabilities: Some(RuntimeCapabilities {
                    memory_snapshot: false,
                    disk_snapshot: true,
                    live_checkpoint: false,
                    guest_agent: true,
                    hardware_isolation: false,
                    lazy_restore: false,
                    cow_fork: false,
                    egress_control: false,
                    full_copy_fork: false,
                    object_store_snapshots: false,
                    capsule_export: false,
                    capsule_import: false,
                    safe_grant_rebind: false,
                }),
                version: "stub".to_string(),
                health: SubstrateHealth::Healthy as i32,
                health_detail: String::new(),
            }],
            total_resources: Some(Resources {
                vcpu: 8,
                mem_mib: 16384,
                disk_mib: 262144,
            }),
            allocatable_resources: Some(Resources {
                vcpu: 6,
                mem_mib: 12288,
                disk_mib: 200000,
            }),
            agent_version: "0.1.0-stub".to_string(),
            // A lone node by construction, which is also the majority real
            // case: no bucket configured, so no membership to report.
            fleet: None,
        }))
    }

    async fn create_instance(&self, _r: Request<CreateInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn start_instance(&self, _r: Request<StartInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn stop_instance(&self, _r: Request<StopInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn pause_instance(&self, _r: Request<PauseInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn resume_instance(&self, _r: Request<ResumeInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn checkpoint_instance(&self, _r: Request<CheckpointInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn destroy_instance(&self, _r: Request<DestroyInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn set_wake(&self, _r: Request<SetWakeRequest>) -> Rsp<Instance> {
        nyi()
    }
    async fn get_instance(&self, _r: Request<GetInstanceRequest>) -> Rsp<Instance> {
        nyi()
    }
    async fn list_instances(
        &self,
        _r: Request<ListInstancesRequest>,
    ) -> Rsp<ListInstancesResponse> {
        nyi()
    }
    async fn list_snapshots(
        &self,
        _r: Request<ListSnapshotsRequest>,
    ) -> Rsp<ListSnapshotsResponse> {
        nyi()
    }
    async fn delete_snapshot(&self, _r: Request<DeleteSnapshotRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn create_snapshot(&self, _r: Request<CreateSnapshotRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn fork_instance(&self, _r: Request<ForkInstanceRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn export_capsule(&self, _r: Request<ExportCapsuleRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn import_capsule(&self, _r: Request<ImportCapsuleRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn delete_capsule(&self, _r: Request<DeleteCapsuleRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn get_capsule(&self, _r: Request<GetCapsuleRequest>) -> Rsp<Capsule> {
        nyi()
    }
    async fn list_capsules(&self, _r: Request<ListCapsulesRequest>) -> Rsp<ListCapsulesResponse> {
        nyi()
    }
    async fn get_operation(&self, _r: Request<GetOperationRequest>) -> Rsp<Operation> {
        nyi()
    }
    async fn cancel_operation(&self, _r: Request<CancelOperationRequest>) -> Rsp<Operation> {
        nyi()
    }

    type WatchEventsStream = Stream<Event>;
    async fn watch_events(&self, _r: Request<WatchEventsRequest>) -> Rsp<Self::WatchEventsStream> {
        nyi()
    }

    type ExecStream = Stream<ExecFrame>;
    async fn exec(&self, _r: Request<Streaming<ExecFrame>>) -> Rsp<Self::ExecStream> {
        nyi()
    }

    type ReadFileStream = Stream<FileChunk>;
    async fn read_file(&self, _r: Request<ReadFileRequest>) -> Rsp<Self::ReadFileStream> {
        nyi()
    }

    async fn write_file(&self, _r: Request<Streaming<WriteFileRequest>>) -> Rsp<WriteFileResponse> {
        nyi()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::args().nth(1).unwrap_or_default().parse()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let bound = listener.local_addr()?.port();
    println!("LISTENING {bound}");

    tonic::transport::Server::builder()
        .add_service(NodeAgentServer::new(Stub))
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await?;
    Ok(())
}
