#[cfg(test)]
mod protocol_tests;

pub mod class_type;
pub mod event_request;
pub mod location;
pub mod object_reference;
pub mod reference_type;
pub mod stack_frame;
pub mod thread_reference;
pub mod value;
pub mod virtual_machine;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{oneshot, watch};
use tokio::time;

use crate::events::{EventBatch, EventQueue};
use crate::wire::{IdSizes, Reader, Writer};

pub const MAX_PACKET: usize = 16 * 1024 * 1024;
pub const EVENT_BUDGET: usize = 4 * 1024 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub trait Command {
    type Response: Response;

    const COMMAND_SET: u8;
    const COMMAND: u8;

    fn encode_request(&self, _writer: &mut Writer, _sizes: IdSizes) -> anyhow::Result<()> {
        Ok(())
    }

    fn decode_response(
        &self,
        reader: &mut Reader<'_>,
        sizes: IdSizes,
    ) -> anyhow::Result<Self::Response> {
        Self::Response::decode(reader, sizes)
    }
}

pub trait Response: Sized {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self>;
}

impl Response for () {
    fn decode(_reader: &mut Reader<'_>, _sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(())
    }
}

impl Response for String {
    fn decode(reader: &mut Reader<'_>, _sizes: IdSizes) -> anyhow::Result<Self> {
        reader.read_string()
    }
}

impl Response for u32 {
    fn decode(reader: &mut Reader<'_>, _sizes: IdSizes) -> anyhow::Result<Self> {
        reader.read_u32()
    }
}

impl<T: Response> Response for Vec<T> {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        (0..reader.read_count()?)
            .map(|_| T::decode(reader, sizes))
            .collect()
    }
}

pub struct ObjectId(pub u64);
pub struct ReferenceTypeId(pub u64);

impl Response for ObjectId {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self(reader.read_id(sizes.object)?))
    }
}

impl Response for ReferenceTypeId {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self(reader.read_id(sizes.reference_type)?))
    }
}

impl Response for IdSizes {
    fn decode(reader: &mut Reader<'_>, _sizes: IdSizes) -> anyhow::Result<Self> {
        Self::decode(reader.read_bytes(20)?)
    }
}

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<Reply>>>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Reply {
    pub jdwp_request_id: u32,
    pub error_code: u16,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jdwp_request_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u16>,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_request_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<TypeCandidate>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TypeCandidate {
    pub type_id: String,
    pub class_loader_id: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Failure {}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            jdwp_request_id: None,
            error_code: None,
            outcome: "not_submitted".into(),
            event_request_id: None,
            candidates: vec![],
        }
    }

    pub fn from_error(error: &anyhow::Error) -> Self {
        if let Some(failure) = error.downcast_ref::<Self>() {
            let mut failure = failure.clone();

            failure.message = error
                .chain()
                .take_while(|cause| !cause.is::<Self>())
                .map(ToString::to_string)
                .chain(std::iter::once(failure.message.clone()))
                .collect::<Vec<_>>()
                .join(": ");

            return failure;
        }

        let mut failure = Self::new("command_error", format!("{error:#}"));

        failure.outcome = "unknown".into();
        failure
    }
}

pub struct JdwpClient {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_id: AtomicU32,
    pub sizes: IdSizes,
    pub queue: Arc<Mutex<EventQueue>>,
    pub disconnected: watch::Sender<Option<String>>,
    pub endpoint: String,
}

struct Packet {
    id: u32,
    flags: u8,
    command_set_or_error_high: u8,
    command_or_error_low: u8,
    payload: Vec<u8>,
}

impl JdwpClient {
    pub async fn connect(host: &str, port: u16) -> anyhow::Result<Arc<Self>> {
        let endpoint = format!("{host}:{port}");
        let mut stream = time::timeout(REQUEST_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .context("connect timeout")??;

        stream.set_nodelay(true)?;
        time::timeout(REQUEST_TIMEOUT, async {
            stream.write_all(b"JDWP-Handshake").await?;

            let mut response = [0; 14];

            stream.read_exact(&mut response).await?;
            ensure!(&response == b"JDWP-Handshake", "invalid JDWP handshake");

            anyhow::Ok(())
        })
        .await
        .context("handshake timeout")??;

        // IDSizes is the only request before the continuous dispatcher starts.
        let mut header = Vec::from(11u32.to_be_bytes());

        header.extend(1u32.to_be_bytes());
        header.extend([
            0,
            virtual_machine::IdSizes::COMMAND_SET,
            virtual_machine::IdSizes::COMMAND,
        ]);
        stream.write_all(&header).await?;

        let (mut reader, writer) = stream.into_split();
        let mut early = VecDeque::new();
        let mut early_bytes = 0;
        let mut early_dropped = 0u64;
        let sizes = time::timeout(REQUEST_TIMEOUT, async {
            loop {
                let packet = read_packet(&mut reader).await?;

                if packet.flags == 0x80 {
                    ensure!(
                        packet.id == 1
                            && packet.command_set_or_error_high == 0
                            && packet.command_or_error_low == 0,
                        "IDSizes failed"
                    );

                    break IdSizes::decode(&packet.payload);
                }

                early_bytes += packet.payload.len() + 11;
                early.push_back(packet);

                while early_bytes > EVENT_BUDGET {
                    early_bytes -= early
                        .pop_front()
                        .expect("early queue is nonempty")
                        .payload
                        .len()
                        + 11;
                    early_dropped += 1;
                }
            }
        })
        .await
        .context("IDSizes timeout")??;
        let queue = Arc::new(Mutex::new(EventQueue::new(EVENT_BUDGET)));

        {
            let mut queue = queue.lock().unwrap();

            queue.dropped = early_dropped;

            for packet in early {
                queue.push(
                    EventBatch::decode(
                        packet.id,
                        packet.command_set_or_error_high,
                        packet.command_or_error_low,
                        &packet.payload,
                        sizes,
                    ),
                    packet.payload.len() + 11,
                );
            }
        }

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (disconnected, _) = watch::channel(None);
        let client = Arc::new(Self {
            writer: tokio::sync::Mutex::new(writer),
            pending: pending.clone(),
            next_id: AtomicU32::new(2),
            sizes,
            queue: queue.clone(),
            disconnected: disconnected.clone(),
            endpoint,
        });
        let mut closed = disconnected.subscribe();

        tokio::spawn(async move {
            let result = tokio::select! {
                result = receive(&mut reader, &pending, &queue, sizes) => result,
                _ = closed.changed() => Ok(()),
            };

            if let Err(error) = result {
                disconnected.send_replace(Some(format!("{error:#}")));
            }

            pending.lock().unwrap().clear();
        });

        Ok(client)
    }

    pub async fn raw(&self, set: u8, command: u8, payload: Vec<u8>) -> anyhow::Result<Reply> {
        ensure!(
            payload.len() <= MAX_PACKET - 11,
            "JDWP payload exceeds 16 MiB packet limit"
        );

        if let Some(reason) = self.disconnected.borrow().clone() {
            return Err(Failure::new("disconnected", reason).into());
        }

        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| anyhow::anyhow!("JDWP request IDs exhausted; attach again"))?;
        let (sender, receiver) = oneshot::channel();
        let mut bytes = Vec::with_capacity(payload.len() + 11);

        bytes.extend(((payload.len() + 11) as u32).to_be_bytes());
        bytes.extend(id.to_be_bytes());
        bytes.extend([0, set, command]);
        bytes.extend(payload);
        self.pending.lock().unwrap().insert(id, sender);

        let write = time::timeout(REQUEST_TIMEOUT, async {
            self.writer.lock().await.write_all(&bytes).await
        })
        .await;

        if !matches!(write, Ok(Ok(()))) {
            self.pending.lock().unwrap().remove(&id);
            self.disconnected.send_replace(Some(
                "JDWP write failed or timed out; packet may be partial".into(),
            ));

            return Err(Failure {
                code: "disconnected".into(),
                message: "JDWP write failed or timed out; connection cannot be reused".into(),
                jdwp_request_id: Some(id),
                error_code: None,
                outcome: "unknown".into(),
                event_request_id: None,
                candidates: vec![],
            }
            .into());
        }

        let result = time::timeout(REQUEST_TIMEOUT, async {
            receiver
                .await
                .context("target disconnected before response")
        })
        .await;

        self.pending.lock().unwrap().remove(&id);

        match result {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(error)) => {
                self.disconnected.send_replace(Some(error.to_string()));

                Err(Failure {
                    code: "disconnected".into(),
                    message: error.to_string(),
                    jdwp_request_id: Some(id),
                    error_code: None,
                    outcome: "unknown".into(),
                    event_request_id: None,
                    candidates: vec![],
                }
                .into())
            }
            Err(_) => Err(Failure {
                code: "request_timeout".into(),
                message: "response not confirmed; do not automatically retry".into(),
                jdwp_request_id: Some(id),
                error_code: None,
                outcome: "unknown".into(),
                event_request_id: None,
                candidates: vec![],
            }
            .into()),
        }
    }

    pub async fn request<T: Command>(&self, request: T) -> anyhow::Result<T::Response> {
        let mut writer = Writer::default();

        request.encode_request(&mut writer, self.sizes)?;

        let reply = self
            .raw(T::COMMAND_SET, T::COMMAND, writer.into_bytes())
            .await?;

        if reply.error_code != 0 {
            return Err(Failure {
                code: "jdwp_error".into(),
                message: format!(
                    "JDWP {}/{} returned {}",
                    T::COMMAND_SET,
                    T::COMMAND,
                    reply.error_code
                ),
                jdwp_request_id: Some(reply.jdwp_request_id),
                error_code: Some(reply.error_code),
                outcome: "rejected".into(),
                event_request_id: None,
                candidates: vec![],
            }
            .into());
        }

        let mut reader = Reader::new(&reply.payload);
        let response = request.decode_response(&mut reader, self.sizes)?;

        reader.finish()?;

        Ok(response)
    }

    pub async fn close(&self) {
        let _ = time::timeout(
            Duration::from_secs(2),
            self.request(virtual_machine::Dispose),
        )
        .await;

        self.disconnected
            .send_replace(Some("session closed".into()));

        let _ = self.writer.lock().await.shutdown().await;
    }
}

async fn read_packet(reader: &mut OwnedReadHalf) -> anyhow::Result<Packet> {
    let mut header = [0; 11];

    reader
        .read_exact(&mut header)
        .await
        .context("JDWP connection closed")?;

    let len = u32::from_be_bytes(header[..4].try_into()?) as usize;

    ensure!(
        (11..=MAX_PACKET).contains(&len),
        "invalid JDWP packet length {len}"
    );
    ensure!(
        header[8] == 0 || header[8] == 0x80,
        "invalid JDWP packet flags"
    );

    let mut payload = vec![0; len - 11];

    reader.read_exact(&mut payload).await?;

    Ok(Packet {
        id: u32::from_be_bytes(header[4..8].try_into()?),
        flags: header[8],
        command_set_or_error_high: header[9],
        command_or_error_low: header[10],
        payload,
    })
}

async fn receive(
    reader: &mut OwnedReadHalf,
    pending: &Pending,
    queue: &Arc<Mutex<EventQueue>>,
    sizes: IdSizes,
) -> anyhow::Result<()> {
    loop {
        let packet = read_packet(reader).await?;

        if packet.flags == 0x80 {
            if let Some(sender) = pending.lock().unwrap().remove(&packet.id) {
                let _ = sender.send(Reply {
                    jdwp_request_id: packet.id,
                    error_code: u16::from_be_bytes([
                        packet.command_set_or_error_high,
                        packet.command_or_error_low,
                    ]),
                    payload: packet.payload,
                });
            }
        } else {
            let batch = EventBatch::decode(
                packet.id,
                packet.command_set_or_error_high,
                packet.command_or_error_low,
                &packet.payload,
                sizes,
            );

            queue.lock().unwrap().push(batch, packet.payload.len() + 11);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn forwarding_failure_preserves_context_without_repeating_code() {
        let mut failure = Failure::new("jdwp_error", "JDWP 11/6 returned 13");
        failure.error_code = Some(13);
        failure.jdwp_request_id = Some(42);
        failure.outcome = "rejected".into();
        let error = anyhow::Error::new(failure).context("read stack");
        let forwarded = Failure::from_error(&error);
        assert_eq!(forwarded.message, "read stack: JDWP 11/6 returned 13");
        let encoded = serde_json::to_string(&forwarded).unwrap();
        let received: Failure = serde_json::from_str(&encoded).unwrap();
        let forwarded = Failure::from_error(&received.into());
        assert_eq!(forwarded.message, "read stack: JDWP 11/6 returned 13");
        assert_eq!(forwarded.error_code, Some(13));
        assert_eq!(forwarded.jdwp_request_id, Some(42));
        assert_eq!(forwarded.outcome, "rejected");
    }

    async fn read_request(stream: &mut TcpStream) -> (u32, u8, u8) {
        let mut header = [0; 11];
        stream.read_exact(&mut header).await.unwrap();
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let mut body = vec![0; len - 11];
        stream.read_exact(&mut body).await.unwrap();
        (
            u32::from_be_bytes(header[4..8].try_into().unwrap()),
            header[9],
            header[10],
        )
    }

    async fn reply(stream: &mut TcpStream, id: u32, payload: &[u8]) {
        let mut bytes = ((payload.len() + 11) as u32).to_be_bytes().to_vec();
        bytes.extend(id.to_be_bytes());
        bytes.extend([0x80, 0, 0]);
        bytes.extend(payload);
        stream.write_all(&bytes).await.unwrap();
    }

    #[tokio::test]
    async fn timeout_late_reply_is_not_reused_for_next_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (late, wait_late) = oneshot::channel();
        let (finished, finish) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut handshake = [0; 14];
            stream.read_exact(&mut handshake).await.unwrap();
            stream.write_all(&handshake).await.unwrap();
            let (id, _, _) = read_request(&mut stream).await;
            reply(&mut stream, id, &[0, 0, 0, 8].repeat(5)).await;
            let (old, _, _) = read_request(&mut stream).await;
            wait_late.await.unwrap();
            reply(&mut stream, old, b"old").await;
            let (new, _, _) = read_request(&mut stream).await;
            assert_ne!(old, new);
            reply(&mut stream, new, b"new").await;
            let _ = finish.await;
        });
        let client = JdwpClient::connect("127.0.0.1", port).await.unwrap();
        time::pause();
        let error = client.raw(100, 1, vec![]).await.unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.code, "request_timeout");
        assert_eq!(failure.outcome, "unknown");
        time::resume();
        late.send(()).unwrap();
        let result = client.raw(100, 2, vec![]).await.unwrap();
        assert_eq!(result.payload, b"new");
        finished.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_packet_disconnects_and_fails_pending_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut handshake = [0; 14];
            stream.read_exact(&mut handshake).await.unwrap();
            stream.write_all(&handshake).await.unwrap();
            let (id, _, _) = read_request(&mut stream).await;
            reply(&mut stream, id, &[0, 0, 0, 4].repeat(5)).await;
            read_request(&mut stream).await;
            stream.write_all(&[0; 11]).await.unwrap();
        });
        let client = JdwpClient::connect("127.0.0.1", port).await.unwrap();
        let error = client.raw(100, 1, vec![]).await.unwrap_err();
        assert_eq!(
            error.downcast_ref::<Failure>().unwrap().code,
            "disconnected"
        );
        assert!(client.disconnected.borrow().is_some());
        server.await.unwrap();
    }
}
