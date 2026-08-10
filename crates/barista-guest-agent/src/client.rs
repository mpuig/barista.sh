//! Reference client for the workload idle surface (barista-031).
//!
//! The whole client surface is one gRPC call on a unix socket, so this is the
//! whole client: a workload in any OCI image can do the same with `grpcurl`, or
//! run `barista-guest-agent declare-idle` — this binary is already in the
//! sandbox as its entrypoint, so it doubles as the zero-dependency client.

use std::path::Path;

use anyhow::{Context, Result};
use barista_proto::guest::v1alpha1::workload_service_client::WorkloadServiceClient;
use barista_proto::guest::v1alpha1::DeclareIdleRequest;
use hyper_util::rt::TokioIo;
use tonic::transport::{Endpoint, Uri};

/// Connect to the workload socket and declare the workload idle.
///
/// Unauthenticated by design: the socket is reachable only from inside this
/// sandbox, whose single trust domain the caller already shares (serve.rs).
pub async fn declare_idle(socket: &Path) -> Result<()> {
    let socket = socket.to_path_buf();
    // The authority is ignored for a unix socket; the connector is what dials.
    let channel = Endpoint::try_from("http://workload.invalid")
        .context("building the workload endpoint")?
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let path = socket.clone();
            async move {
                Ok::<_, std::io::Error>(TokioIo::new(tokio::net::UnixStream::connect(path).await?))
            }
        }))
        .await
        .with_context(|| "connecting to the workload socket")?;
    WorkloadServiceClient::new(channel)
        .declare_idle(DeclareIdleRequest {})
        .await
        .context("calling DeclareIdle on the workload socket")?;
    Ok(())
}
