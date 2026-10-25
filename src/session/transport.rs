use std::future::Future;
use std::io::{Cursor, Error, ErrorKind};
use std::pin::Pin;

use anyhow::{Context, anyhow};
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, TryStreamExt};
use rmp_serde::Deserializer as MessagePackDeserializer;
use serde::{Serialize, de::DeserializeOwned};
use tokio::net::UnixStream;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_serde::{Deserializer, Framed, Serializer};
use tokio_util::codec::{Framed as ByteFrames, LengthDelimitedCodec};

const MAX_FRAME_LENGTH: usize = 64 * 1024 * 1024;

struct MessagePack;

impl<T: Serialize> Serializer<T> for MessagePack {
    type Error = Error;

    fn serialize(self: Pin<&mut Self>, message: &T) -> Result<Bytes, Self::Error> {
        // Field names preserve omitted optional fields in shared JSONL types.
        rmp_serde::to_vec_named(message)
            .map(Bytes::from)
            .map_err(Error::other)
    }
}

impl<T: DeserializeOwned> Deserializer<T> for MessagePack {
    type Error = Error;

    fn deserialize(self: Pin<&mut Self>, bytes: &BytesMut) -> Result<T, Self::Error> {
        let mut decoder = MessagePackDeserializer::new(Cursor::new(bytes.as_ref()));
        let message = T::deserialize(&mut decoder).map_err(Error::other)?;

        if decoder.position() != bytes.len() as u64 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "trailing bytes in IPC frame",
            ));
        }

        Ok(message)
    }
}

pub trait Transport: Send {
    type Outgoing: Send;
    type Incoming: Send;

    fn send(&mut self, message: Self::Outgoing) -> impl Future<Output = anyhow::Result<()>> + Send;

    // Cancellation must preserve partial frames and queued messages.
    fn receive(&mut self) -> impl Future<Output = anyhow::Result<Option<Self::Incoming>>> + Send;
}

pub struct Socket<Outgoing, Incoming> {
    framed: Framed<ByteFrames<UnixStream, LengthDelimitedCodec>, Incoming, Outgoing, MessagePack>,
}

impl<Outgoing, Incoming> Socket<Outgoing, Incoming> {
    pub fn new(stream: UnixStream) -> Self {
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME_LENGTH)
            .new_codec();
        let frames = ByteFrames::new(stream, codec);

        Self {
            framed: Framed::new(frames, MessagePack),
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
    sender: Sender<Outgoing>,
    receiver: Receiver<Incoming>,
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
            .map_err(|_| anyhow!("session connection closed"))
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

    #[test]
    fn file_payload_is_binary_and_json_stays_hex() {
        use crate::commands::CommandResult;
        use crate::session::protocol::ServerMessage;
        use tokio_serde::{Deserializer, Serializer};

        let payload: Vec<u8> = (0..=255).cycle().take(4096).collect();
        let message = ServerMessage::Result {
            result: CommandResult::File {
                bytes: payload.clone(),
            },
        };
        let encoded = Pin::new(&mut MessagePack).serialize(&message).unwrap();
        assert!(encoded.len() < payload.len() + 100);
        assert!(encoded.windows(payload.len()).any(|bytes| bytes == payload));
        let decoded: ServerMessage = Pin::new(&mut MessagePack)
            .deserialize(&BytesMut::from(encoded.as_ref()))
            .unwrap();
        let ServerMessage::Result {
            result: CommandResult::File { bytes },
        } = decoded
        else {
            panic!("expected file response");
        };
        assert_eq!(bytes, payload);
        for payload in [b"".as_slice(), b"AA55", b"not hex"] {
            let message = CommandResult::File {
                bytes: payload.to_vec(),
            };
            let encoded = Pin::new(&mut MessagePack).serialize(&message).unwrap();
            let decoded: CommandResult = Pin::new(&mut MessagePack)
                .deserialize(&BytesMut::from(encoded.as_ref()))
                .unwrap();
            assert!(matches!(decoded, CommandResult::File { bytes } if bytes == payload));
        }
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["result"]["bytes"], crate::wire::hex(&payload));
        let decoded: ServerMessage = serde_json::from_value(json).unwrap();
        assert!(matches!(decoded, ServerMessage::Result {
            result: CommandResult::File { bytes }
        } if bytes == payload));
        assert!(
            <MessagePack as Deserializer<ServerMessage>>::deserialize(
                Pin::new(&mut MessagePack),
                &BytesMut::from(&encoded[..encoded.len() - 1]),
            )
            .is_err()
        );
    }

    #[test]
    fn codec_rejects_trailing_values_and_garbage() {
        use tokio_serde::{Deserializer, Serializer};

        let encoded = Pin::new(&mut MessagePack).serialize(&42u32).unwrap();
        for suffix in [&[0x01][..], &[0xc1][..]] {
            let mut frame = BytesMut::from(encoded.as_ref());
            frame.extend_from_slice(suffix);
            let result: Result<u32, _> = Pin::new(&mut MessagePack).deserialize(&frame);
            assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidData);
        }
    }

    #[test]
    fn raw_response_uses_binary_ipc_and_hex_json() {
        use crate::commands::{CommandResult, Data};
        use crate::session::protocol::ServerMessage;
        use tokio_serde::{Deserializer, Serializer};

        let payload = b"AA55".repeat(1024);
        let message = ServerMessage::Result {
            result: CommandResult::Data {
                data: Data::Raw {
                    jdwp_request_id: 7,
                    error_code: 0,
                    payload: payload.clone(),
                },
            },
        };
        let encoded = Pin::new(&mut MessagePack).serialize(&message).unwrap();
        assert!(encoded.len() < payload.len() + 200);
        assert!(encoded.windows(payload.len()).any(|bytes| bytes == payload));
        let decoded: ServerMessage = Pin::new(&mut MessagePack)
            .deserialize(&BytesMut::from(encoded.as_ref()))
            .unwrap();
        assert!(matches!(&decoded, ServerMessage::Result {
            result: CommandResult::Data { data: Data::Raw { payload: actual, .. } }
        } if actual == &payload));
        let json = serde_json::to_value(decoded).unwrap();
        assert_eq!(
            json["result"]["data"]["payload"],
            crate::wire::hex(&payload)
        );
        assert!(json["result"]["data"].get("payload_hex").is_none());
    }

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
