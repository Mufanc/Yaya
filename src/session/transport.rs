use std::future::Future;

use anyhow::Context;
use futures_util::{SinkExt, TryStreamExt};
use serde::{Serialize, de::DeserializeOwned};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_serde::Framed;
use tokio_serde::formats::Json;

use tokio_util::codec::{Framed as ByteFrames, LengthDelimitedCodec};

const MAX_FRAME_LENGTH: usize = 64 * 1024 * 1024;

pub trait Transport: Send {
    type Outgoing: Send;
    type Incoming: Send;

    fn send(&mut self, message: Self::Outgoing) -> impl Future<Output = anyhow::Result<()>> + Send;

    // Cancellation must preserve partial frames and queued messages.
    fn receive(&mut self) -> impl Future<Output = anyhow::Result<Option<Self::Incoming>>> + Send;
}

pub struct Socket<Outgoing, Incoming> {
    framed: Framed<
        ByteFrames<UnixStream, LengthDelimitedCodec>,
        Incoming,
        Outgoing,
        Json<Incoming, Outgoing>,
    >,
}

impl<Outgoing, Incoming> Socket<Outgoing, Incoming> {
    pub fn new(stream: UnixStream) -> Self {
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME_LENGTH)
            .new_codec();
        let frames = ByteFrames::new(stream, codec);

        Self {
            framed: Framed::new(frames, Json::default()),
        }
    }
}

impl<Outgoing, Incoming> Transport for Socket<Outgoing, Incoming>
where
    Outgoing: Serialize + Send + Unpin,
    Incoming: DeserializeOwned + Send + Unpin,
{
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn send(&mut self, message: Outgoing) -> anyhow::Result<()> {
        Ok(self.framed.send(message).await?)
    }

    async fn receive(&mut self) -> anyhow::Result<Option<Incoming>> {
        Ok(self.framed.try_next().await?)
    }
}

pub struct Channel<Outgoing, Incoming> {
    sender: mpsc::Sender<Outgoing>,
    receiver: mpsc::Receiver<Incoming>,
}

pub fn channel<Outgoing, Incoming>() -> (Channel<Outgoing, Incoming>, Channel<Incoming, Outgoing>) {
    let (request_sender, request_receiver) = mpsc::channel(1);
    let (response_sender, response_receiver) = mpsc::channel(1);

    (
        Channel {
            sender: request_sender,
            receiver: response_receiver,
        },
        Channel {
            sender: response_sender,
            receiver: request_receiver,
        },
    )
}

impl<Outgoing: Send, Incoming: Send> Transport for Channel<Outgoing, Incoming> {
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn send(&mut self, message: Outgoing) -> anyhow::Result<()> {
        self.sender
            .send(message)
            .await
            .map_err(|_| anyhow::anyhow!("session connection closed"))
    }

    async fn receive(&mut self) -> anyhow::Result<Option<Incoming>> {
        Ok(self.receiver.recv().await)
    }
}

pub async fn receive<T: Transport>(transport: &mut T) -> anyhow::Result<T::Incoming> {
    transport
        .receive()
        .await?
        .context("session connection closed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn channel_applies_backpressure_and_drains_before_eof() {
        let (mut client, mut server) = channel::<u64, ()>();
        client.send(1).await.unwrap();
        {
            let blocked = client.send(2);
            tokio::pin!(blocked);
            assert!(futures_util::poll!(&mut blocked).is_pending());
            assert_eq!(server.receive().await.unwrap(), Some(1));
            blocked.await.unwrap();
        }
        drop(client);
        assert_eq!(server.receive().await.unwrap(), Some(2));
        assert_eq!(server.receive().await.unwrap(), None);
        assert!(server.send(()).await.is_err());
    }
}
