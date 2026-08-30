//! Complete instance inventory over Contract A's bounded pages.

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use tonic::transport::Channel;

pub(crate) async fn list_all(
    client: &mut NodeAgentClient<Channel>,
) -> Result<Vec<pb::Instance>, tonic::Status> {
    let mut instances = Vec::new();
    let mut page_token = String::new();
    loop {
        let response = client
            .list_instances(pb::ListInstancesRequest {
                page_size: 256,
                page_token,
                ..Default::default()
            })
            .await?
            .into_inner();
        instances.extend(response.instances);
        if response.next_page_token.is_empty() {
            return Ok(instances);
        }
        page_token = response.next_page_token;
    }
}
