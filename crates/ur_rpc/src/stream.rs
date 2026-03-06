//! Side-channel streaming helpers for `CommandOutput`.
//!
//! Both server (urd) and client (agent_tools) use these to send/receive
//! length-delimited bincode frames of [`CommandOutput`] over a dedicated
//! Unix domain socket.

use std::io;
use std::path::Path;

use futures::{SinkExt, StreamExt};
use tokio::net::{UnixListener, UnixStream};
use tokio_serde::Framed as SerdeFramed;
use tokio_serde::formats::Bincode;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::CommandOutput;

/// Type alias for a framed sender of `CommandOutput` over a Unix stream.
type CommandSink = SerdeFramed<
    Framed<UnixStream, LengthDelimitedCodec>,
    CommandOutput, // Item (unused for sink)
    CommandOutput, // SinkItem
    Bincode<CommandOutput, CommandOutput>,
>;

/// Type alias for a framed receiver of `CommandOutput` over a Unix stream.
pub type CommandStream = SerdeFramed<
    Framed<UnixStream, LengthDelimitedCodec>,
    CommandOutput, // Item
    CommandOutput, // SinkItem (unused for stream)
    Bincode<CommandOutput, CommandOutput>,
>;

/// Server-side: bind a Unix listener on `socket_path`.
///
/// Call this before returning the socket path to the client, then use
/// [`accept_stream_sink`] to accept the connection.
pub fn bind_stream_listener(socket_path: &Path) -> io::Result<UnixListener> {
    UnixListener::bind(socket_path)
}

/// Server-side: accept one connection on a pre-bound listener and return
/// a sink for sending `CommandOutput` frames.
pub async fn accept_stream_sink(listener: UnixListener) -> io::Result<CommandSink> {
    let (stream, _addr) = listener.accept().await?;
    Ok(wrap_sink(stream))
}

/// Client-side: connect to the stream socket and return a stream of `CommandOutput`.
pub async fn connect_stream(socket_path: &Path) -> io::Result<CommandStream> {
    let stream = UnixStream::connect(socket_path).await?;
    Ok(wrap_stream(stream))
}

/// Send a single `CommandOutput` frame on the sink.
pub async fn send_output(sink: &mut CommandSink, output: CommandOutput) -> io::Result<()> {
    sink.send(output).await.map_err(io::Error::other)
}

/// Receive the next `CommandOutput` frame from the stream.
/// Returns `None` when the stream is closed.
pub async fn recv_output(stream: &mut CommandStream) -> Option<io::Result<CommandOutput>> {
    stream.next().await.map(|r| r.map_err(io::Error::other))
}

fn wrap_sink(stream: UnixStream) -> CommandSink {
    let framed = Framed::new(stream, LengthDelimitedCodec::new());
    SerdeFramed::new(framed, Bincode::default())
}

fn wrap_stream(stream: UnixStream) -> CommandStream {
    let framed = Framed::new(stream, LengthDelimitedCodec::new());
    SerdeFramed::new(framed, Bincode::default())
}
