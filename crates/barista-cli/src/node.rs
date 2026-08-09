//! Reaching a node.
//!
//! One flag covers both transports the Node Agent serves, because a caller
//! should not have to know which this node uses — `--node 127.0.0.1:7070` and
//! `--node /run/barista/node.sock` are the same request to a different door
//! (nap-006 design decision 5).

use std::path::PathBuf;

use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use tonic::transport::{Channel, Endpoint, Uri};

/// Where a node is, as the user described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Address {
    Tcp(String),
    Uds(PathBuf),
}

impl Address {
    /// Anything containing a `/` is a path; everything else is `host:port`.
    ///
    /// Crude on purpose. The alternative — a scheme prefix — makes the common
    /// case (`--node 127.0.0.1:7070`) longer to type for no benefit, and a unix
    /// socket path without a slash is not a path anyone writes.
    pub(crate) fn parse(raw: &str) -> Self {
        if raw.contains('/') {
            Address::Uds(PathBuf::from(raw))
        } else {
            Address::Tcp(raw.to_string())
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::Tcp(addr) => write!(f, "{addr}"),
            Address::Uds(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Connect, or explain what could not be reached.
///
/// The error names the address and the likely cause: "connection refused" on its
/// own sends someone to the wrong place, and the node being down is by far the
/// most common reason a CLI call fails.
pub(crate) async fn connect(address: &Address) -> anyhow::Result<NodeAgentClient<Channel>> {
    let channel = match address {
        Address::Tcp(addr) => Endpoint::try_from(format!("http://{addr}"))?
            .connect()
            .await
            .map_err(|e| unreachable(address, e))?,
        Address::Uds(path) => {
            let path = path.clone();
            // The authority is ignored for a unix socket but tonic requires a
            // well-formed URI, so this is a placeholder rather than a destination.
            Endpoint::try_from("http://node.invalid")?
                .connect_with_connector(tower::service_fn(move |_: Uri| {
                    let path = path.clone();
                    async move {
                        tokio::net::UnixStream::connect(path)
                            .await
                            .map(hyper_util::rt::TokioIo::new)
                    }
                }))
                .await
                .map_err(|e| unreachable(address, e))?
        }
    };
    Ok(NodeAgentClient::new(channel))
}

fn unreachable(address: &Address, source: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!(
        "cannot reach a barista node at {address} ({source}).\n\
         Is `barista-node-agent` running? Set --node or BARISTA_NODE to point somewhere else."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_a_socket_and_a_host_port_is_tcp() {
        assert_eq!(
            Address::parse("/run/barista/node.sock"),
            Address::Uds(PathBuf::from("/run/barista/node.sock"))
        );
        assert_eq!(
            Address::parse("./node.sock"),
            Address::Uds("./node.sock".into())
        );
        assert_eq!(
            Address::parse("127.0.0.1:7070"),
            Address::Tcp("127.0.0.1:7070".into())
        );
        assert_eq!(
            Address::parse("node-1.internal:7070"),
            Address::Tcp("node-1.internal:7070".into())
        );
    }
}
