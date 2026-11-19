use std::pin::Pin;

use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Response, Status, Streaming};

use ur_rpc::proto::builder::BuilderdClient;
use ur_rpc::proto::builder::builder_exec_message::Payload as BuilderPayload;
use ur_rpc::proto::builder::{BuilderExecMessage, BuilderExecRequest};
use ur_rpc::proto::core::CommandOutput;
use ur_rpc::proto::hostexec::HostExecMessage;
use ur_rpc::proto::hostexec::host_exec_message::Payload as HostExecPayload;

use crate::HostExecError;

pub type CommandOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<CommandOutput, Status>> + Send>>;

/// Forward stdin frames from the client inbound stream to the builder channel.
async fn forward_stdin_to_builder(
    mut inbound: Streaming<HostExecMessage>,
    tx: mpsc::Sender<BuilderExecMessage>,
) {
    while let Some(Ok(msg)) = inbound.next().await {
        let Some(HostExecPayload::Stdin(data)) = msg.payload else {
            continue;
        };
        let builder_msg = BuilderExecMessage {
            payload: Some(BuilderPayload::Stdin(data)),
        };
        if tx.send(builder_msg).await.is_err() {
            break;
        }
    }
}

pub async fn forward_to_builderd(
    builderd_client: &BuilderdClient,
    transform_result: crate::lua_transform::TransformResult,
    long_lived: bool,
    is_bidi: bool,
    inbound: Streaming<HostExecMessage>,
) -> Result<Response<CommandOutputStream>, Status> {
    let mut client = builderd_client.clone();

    let builder_req = BuilderExecRequest {
        command: transform_result.command,
        args: transform_result.args,
        working_dir: transform_result.working_dir,
        env: transform_result.env,
        long_lived,
    };

    let start_msg = BuilderExecMessage {
        payload: Some(BuilderPayload::Start(builder_req)),
    };

    let (builder_tx, builder_rx) = mpsc::channel::<BuilderExecMessage>(32);
    builder_tx
        .send(start_msg)
        .await
        .map_err(|_| HostExecError::BuilderdExecFailed {
            message: "failed to enqueue start frame".into(),
        })?;

    if is_bidi {
        let tx = builder_tx.clone();
        tokio::spawn(forward_stdin_to_builder(inbound, tx));
    }

    drop(builder_tx);

    let builder_stream = ReceiverStream::new(builder_rx);

    let response =
        client
            .exec(builder_stream)
            .await
            .map_err(|e| HostExecError::BuilderdExecFailed {
                message: e.to_string(),
            })?;

    let mut builderd_inbound = response.into_inner();
    let (tx, rx) = mpsc::channel(32);

    tokio::spawn(async move {
        while let Ok(Some(msg)) = builderd_inbound.message().await {
            if tx.send(Ok(msg)).await.is_err() {
                break;
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    Ok(Response::new(Box::pin(stream) as CommandOutputStream))
}
