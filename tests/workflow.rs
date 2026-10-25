use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

type RecordedRequests = Arc<Mutex<Vec<(u8, u8, Vec<u8>)>>>;

struct Fixture {
    temp: PathBuf,
    port: u16,
    target: Arc<Mutex<Option<TcpStream>>>,
    requests: RecordedRequests,
}

fn u32_bytes(value: u32) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}
fn string(value: &str) -> Vec<u8> {
    let mut bytes = u32_bytes(value.len() as u32);
    bytes.extend(value.as_bytes());
    bytes
}

fn packet(
    id: u32,
    flags: u8,
    command_set_or_error_high: u8,
    command_or_error_low: u8,
    body: &[u8],
) -> Vec<u8> {
    let mut bytes = u32_bytes((11 + body.len()) as u32);
    bytes.extend(id.to_be_bytes());
    bytes.extend([flags, command_set_or_error_high, command_or_error_low]);
    bytes.extend(body);
    bytes
}

fn event(kind: u8, request: u32) -> Vec<u8> {
    let mut body = vec![1];
    body.extend(1u32.to_be_bytes());
    body.push(kind);
    body.extend(request.to_be_bytes());
    body.extend(0x12u64.to_be_bytes());
    if matches!(kind, 1 | 2 | 4 | 20 | 21) {
        body.push(1);
        body.extend(0x101u64.to_be_bytes());
        body.extend(0x82u64.to_be_bytes());
        body.extend(0x19u64.to_be_bytes());
    }
    match kind {
        4 => {
            body.push(b'L');
            body.extend(0x55u64.to_be_bytes());
            body.extend([0; 25]);
        }
        8 => {
            body.push(1);
            body.extend(0x101u64.to_be_bytes());
            body.extend(string("Ltest;"));
            body.extend(2u32.to_be_bytes());
        }
        20 | 21 => {
            body.push(1);
            body.extend(0x101u64.to_be_bytes());
            body.extend(0x45u64.to_be_bytes());
            body.push(b'L');
            body.extend(0x55u64.to_be_bytes());
            if kind == 21 {
                body.push(b'I');
                body.extend(86i32.to_be_bytes());
            }
        }
        _ => (),
    }
    packet(400, 0, 64, 100, &body)
}

impl Fixture {
    fn new() -> Self {
        Self::with_value_count(1)
    }

    fn with_value_count(value_count: u32) -> Self {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let temp = PathBuf::from(format!(
            "/tmp/yy-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&temp).unwrap();
        let target = Arc::new(Mutex::new(None));
        let shared = target.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            *shared.lock().unwrap() = Some(stream.try_clone().unwrap());
            let mut handshake = [0; 14];
            stream.read_exact(&mut handshake).unwrap();
            assert_eq!(&handshake, b"JDWP-Handshake");
            stream.write_all(b"JDWP-").unwrap();
            stream.write_all(b"Handshake").unwrap();
            // VMStart before IDSizes must not deadlock or be lost.
            stream.write_all(&event(90, 0)).unwrap();
            let mut suspend_count = 0u32;
            let mut array_values = [10i32, 20, 30];
            loop {
                let mut header = [0; 11];
                if stream.read_exact(&mut header).is_err() {
                    break;
                }
                let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
                let id = u32::from_be_bytes(header[4..8].try_into().unwrap());
                let (set, command) = (header[9], header[10]);
                let mut body = vec![0; len - 11];
                stream.read_exact(&mut body).unwrap();
                recorded.lock().unwrap().push((set, command, body.clone()));
                let mut reply = Vec::new();
                let mut error = 0;
                match (set, command) {
                    (1, 7) => {
                        for _ in 0..5 {
                            reply.extend(8u32.to_be_bytes());
                        }
                    }
                    (1, 4) => {
                        reply.extend(1u32.to_be_bytes());
                        reply.extend(0x12u64.to_be_bytes());
                    }
                    (10, 1) => reply = string("hello 你好"),
                    (1, 8) | (11, 2) => suspend_count += 1,
                    (11, 12) => reply.extend(suspend_count.to_be_bytes()),
                    (13, 1) => reply.extend(3u32.to_be_bytes()),
                    (13, 2) => {
                        let start = u32::from_be_bytes(body[8..12].try_into().unwrap()) as usize;
                        let length = u32::from_be_bytes(body[12..16].try_into().unwrap()) as usize;
                        if start + length > array_values.len() {
                            error = 103;
                        } else {
                            reply.push(b'I');
                            reply.extend((length as u32).to_be_bytes());
                            for value in &array_values[start..start + length] {
                                reply.extend(value.to_be_bytes());
                            }
                        }
                    }
                    (13, 3) => {
                        let start = u32::from_be_bytes(body[8..12].try_into().unwrap()) as usize;
                        let count = u32::from_be_bytes(body[12..16].try_into().unwrap()) as usize;
                        if start + count > array_values.len() {
                            error = 103;
                        } else {
                            assert_eq!(body.len(), 16 + count * 4);
                            for (index, bytes) in body[16..].chunks_exact(4).enumerate() {
                                array_values[start + index] =
                                    i32::from_be_bytes(bytes.try_into().unwrap());
                            }
                        }
                    }
                    (9, 10) => {
                        reply.extend(1u32.to_be_bytes());
                        reply.push(b'L');
                        reply.extend(0x88u64.to_be_bytes());
                    }
                    (16, 3) => {
                        reply.push(b'L');
                        reply.extend(0x55u64.to_be_bytes());
                    }
                    (11, 1) => reply = string("main"),
                    (11, 4) => {
                        reply.extend(1u32.to_be_bytes());
                        reply.extend(1u32.to_be_bytes());
                    }
                    (1, 2) => {
                        reply.extend(1u32.to_be_bytes());
                        reply.push(1);
                        reply.extend(0x101u64.to_be_bytes());
                        reply.extend(7u32.to_be_bytes());
                    }
                    (2, 2) => reply.extend(0x31u64.to_be_bytes()),
                    (9, 1) => {
                        reply.push(1);
                        reply.extend(0x101u64.to_be_bytes());
                    }
                    (2, 1) => reply = string("Ltest;"),
                    (2, 4) => {
                        reply.extend(1u32.to_be_bytes());
                        reply.extend(0x45u64.to_be_bytes());
                        reply.extend(string("height"));
                        reply.extend(string("I"));
                        reply.extend(0u32.to_be_bytes());
                    }
                    (3, 1) => reply.extend(0u64.to_be_bytes()),
                    (2, 10) => reply.extend(0u32.to_be_bytes()),
                    (9, 2) => {
                        reply.extend(value_count.to_be_bytes());
                        for _ in 0..value_count {
                            reply.push(b'I');
                            reply.extend(85u32.to_be_bytes());
                        }
                    }
                    (9, 3) => (),
                    (2, 5) => {
                        reply.extend(1u32.to_be_bytes());
                        reply.extend(0x82u64.to_be_bytes());
                        reply.extend(string("foo"));
                        reply.extend(string("()Z"));
                        reply.extend(1u32.to_be_bytes());
                    }
                    (15, 1) => {
                        reply.extend(77u32.to_be_bytes());
                        if matches!(body[0], 4 | 8 | 20 | 21) {
                            stream.write_all(&event(body[0], 77)).unwrap();
                        }
                    }
                    (11, 3) => {
                        suspend_count = suspend_count.saturating_sub(1);
                        stream.write_all(&event(1, 77)).unwrap();
                    }
                    (11, 6) => {
                        reply.extend(1u32.to_be_bytes());
                        reply.extend(0x22u64.to_be_bytes());
                        reply.push(1);
                        reply.extend(0x101u64.to_be_bytes());
                        reply.extend(0x82u64.to_be_bytes());
                        reply.extend(0x19u64.to_be_bytes());
                    }
                    (16, 1) => {
                        reply.extend(value_count.to_be_bytes());
                        for _ in 0..value_count {
                            reply.push(b'I');
                            reply.extend(42u32.to_be_bytes());
                        }
                    }
                    (1, 9) => suspend_count = suspend_count.saturating_sub(1),
                    (16, 2) | (15, 2) | (15, 3) => (),
                    (1, 6) => {
                        stream.write_all(&packet(id, 0x80, 0, 0, &[])).unwrap();
                        break;
                    }
                    (199, 1) => {
                        assert!(body.len() >= 8);
                        assert_eq!(
                            u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize,
                            body.len() - 8
                        );
                        let chunk = &body[..4];
                        let mut payload = Vec::new();
                        let mut response_chunk = chunk;
                        match chunk {
                            b"FEAT" | b"VULW" => {
                                payload.extend(1u32.to_be_bytes());
                                let text = if chunk == b"FEAT" {
                                    "view-hierarchy"
                                } else {
                                    "window"
                                };
                                payload.extend((text.len() as u32).to_be_bytes());
                                for unit in text.encode_utf16() {
                                    payload.extend(unit.to_be_bytes());
                                }
                            }
                            b"VURT" => payload.extend(include_bytes!("fixtures/view-v2.bin")),
                            b"VUOP" => {
                                if body.ends_with(b"\0b\0a\0d") {
                                    payload.extend(b"not a PNG");
                                } else {
                                    payload.extend(include_bytes!("fixtures/capture.png"));
                                }
                            }
                            b"FAIL" => {
                                payload.extend((-2i32).to_be_bytes());
                                payload.extend(1u32.to_be_bytes());
                                payload.extend([0, b'x']);
                            }
                            b"JDWE" => {
                                error = 99;
                                response_chunk = b"";
                            }
                            _ => response_chunk = b"",
                        }
                        if !response_chunk.is_empty() {
                            reply.extend(response_chunk);
                            reply.extend((payload.len() as u32).to_be_bytes());
                            reply.extend(payload);
                        }
                    }
                    (100, 1) => {
                        thread::sleep(Duration::from_millis(150));
                        reply = body;
                    }
                    _ => error = 99,
                }
                if stream
                    .write_all(&packet(id, 0x80, 0, error, &reply))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            temp,
            port,
            target,
            requests,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yaya"));
        command.env("TMPDIR", &self.temp).env_remove("YAYA_SESSION");
        command
    }

    fn run(&self, args: &[&str]) -> (Output, Vec<Value>) {
        let output = self.command().args(args).output().unwrap();
        let records = String::from_utf8(output.stdout.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (output, records)
    }

    fn attach(&self) -> String {
        let (output, records) = self.run(&["attach", "--port", &self.port.to_string()]);
        assert!(
            output.status.success(),
            "{records:?} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        records[0]["session"].as_str().unwrap().to_owned()
    }

    fn run_commands(&self, interactive: bool, commands: &[&str]) -> Vec<Value> {
        if interactive {
            let mut child = self
                .command()
                .args(["attach", "-p", &self.port.to_string(), "-i"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            for command in commands {
                writeln!(input, "{command}").unwrap();
            }
            writeln!(input, "exit").unwrap();
            drop(input);
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        } else {
            let session = self.attach();
            let mut records = Vec::new();
            for command in commands {
                let mut args: Vec<_> = command.split_whitespace().collect();
                args.extend(["-s", &session]);
                records.extend(self.run(&args).1);
            }
            assert!(self.run(&["detach", "-s", &session]).0.status.success());
            records
        }
    }

    fn ipc(&self, id: &str) -> UnixStream {
        let path = self.temp.join("yaya").join(id).join("control.sock");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut socket = UnixStream::connect(&path).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            send(
                &mut socket,
                json!({"kind":"hello","version":10,"session":id}),
            );
            let response = recv(&mut socket);
            if response["kind"] == "ready" {
                assert_eq!(response["version"], 10);
                return socket;
            }
            assert!(Instant::now() < deadline, "{response}");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stream) = self.target.lock().unwrap().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let root = self.temp.join("yaya");
            if fs::read_dir(&root).map_or(true, |mut entries| entries.next().is_none()) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_dir_all(&self.temp);
    }
}

fn send(socket: &mut UnixStream, value: Value) {
    let bytes = rmp_serde::to_vec_named(&value).unwrap();
    socket
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&bytes).unwrap();
}

fn recv(socket: &mut UnixStream) -> Value {
    let mut len = [0; 4];
    socket.read_exact(&mut len).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(len) as usize];
    socket.read_exact(&mut bytes).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

#[test]
fn daemon_debugging_workflow() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let (output, records) = fixture.run(&["thread", "list", "--session", &id]);
    assert!(output.status.success(), "{records:?}");
    assert_eq!(records[0]["data"]["threads"][0]["name"], "main");
    let (output, records) = fixture.run(&["events", "--session", &id]);
    assert!(output.status.success(), "{records:?}");
    assert_eq!(records[0]["batch"]["events"][0]["kind"], "vm_started");
    let (output, records) = fixture.run(&[
        "break",
        "--session",
        &id,
        "add",
        "Ltest;->foo()Z",
        "--pc",
        "0x19",
    ]);
    assert!(output.status.success(), "{records:?}");
    assert_eq!(records[0]["data"]["event_request_id"], 77);
    let (output, records) = fixture.run(&[
        "thread",
        "step",
        "--session",
        &id,
        "--thread",
        "0x12",
        "--over",
    ]);
    assert!(output.status.success(), "{records:?}");
    let (output, records) = fixture.run(&["events", "--session", &id, "--wait", "2s"]);
    assert!(output.status.success(), "{records:?}");
    assert_eq!(records[0]["batch"]["events"][0]["kind"], "step_completed");
    let (_, records) = fixture.run(&["thread", "stack", "--session", &id, "-t", "0x12"]);
    let frame = records[0]["data"]["frames"][0]["frame"].as_str().unwrap();
    let (output, records) = fixture.run(&[
        "frame",
        "--session",
        &id,
        "get",
        frame,
        "-t",
        "0x12",
        "--registers",
        "v0:int",
    ]);
    assert!(output.status.success(), "{records:?}");
    assert_eq!(records[0]["data"]["registers"][0][1]["value"], "42");
    let request = fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|(set, cmd, _)| (*set, *cmd) == (16, 1))
        .unwrap()
        .2
        .clone();
    assert_eq!(&request[20..24], &0u32.to_be_bytes());
    assert_eq!(&request[..8], &0x12u64.to_be_bytes());
    assert_eq!(&request[8..16], &0x22u64.to_be_bytes());
    let (output, rows) = fixture.run(&[
        "frame", "set", frame, "-t", "0x12", "v0", "--type", "int", "--value", "42", "-s", &id,
    ]);
    assert!(output.status.success(), "{rows:?}");
    let calls = fixture.requests.lock().unwrap();
    let write = &calls
        .iter()
        .find(|(set, cmd, _)| (*set, *cmd) == (16, 2))
        .unwrap()
        .2;
    assert_eq!(&write[..8], &0x12u64.to_be_bytes());
    assert_eq!(&write[8..16], &0x22u64.to_be_bytes());
    assert_eq!(&write[write.len() - 4..], &42i32.to_be_bytes());
    drop(calls);
    let (output, records) = fixture.run(&["detach", "--session", &id]);
    assert!(output.status.success(), "{records:?}");
    assert!(
        !fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(set, cmd, _)| (*set, *cmd) == (1, 10))
    );
}

#[test]
fn old_protocol_is_rejected_before_commands() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let path = fixture.temp.join("yaya").join(&id).join("control.sock");
    let mut socket = UnixStream::connect(path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    send(
        &mut socket,
        json!({"kind":"hello","version":9,"session":id}),
    );
    let mut byte = [0];
    assert_eq!(
        socket.read(&mut byte).unwrap(),
        0,
        "old protocol must not receive Ready"
    );
    drop(socket);
    let mut current = fixture.ipc(&id);
    send(
        &mut current,
        json!({"kind":"execute","command":{"Thread":{"List":null}}}),
    );
    assert_eq!(recv(&mut current)["kind"], "result");
    drop(current);
}

#[test]
fn ack_disconnect_replays_whole_batch_and_rejects_second_client() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let mut first = fixture.ipc(&id);
    send(&mut first, json!({"kind":"subscribe","drain":true}));
    let original = recv(&mut first);
    assert_eq!(original["kind"], "delivery");
    let (output, records) = fixture.run(&["status", "--session", &id]);
    assert!(!output.status.success());
    assert_eq!(records[0]["error"]["code"], "session_busy");
    drop(first);
    let mut second = fixture.ipc(&id);
    send(&mut second, json!({"kind":"subscribe","drain":true}));
    let replay = recv(&mut second);
    assert_eq!(original, replay);
    assert!(
        !serde_json::to_string(&replay)
            .unwrap()
            .contains("wire_bytes")
    );
    send(
        &mut second,
        json!({"kind":"ack","sequence":replay["delivery"]["sequence"]}),
    );
    assert_eq!(recv(&mut second)["kind"], "drained");
}

#[test]
fn submitted_operation_survives_client_disconnect() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let mut first = fixture.ipc(&id);
    send(
        &mut first,
        json!({"kind":"execute","command":{"Jdwp":{"Raw":{"command_set":100,"command":1,"payload_hex":"aabb"}}}}),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while !fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|(set, _, _)| *set == 100)
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    drop(first);
    let mut next = fixture.ipc(&id);
    send(
        &mut next,
        json!({"kind":"execute","command":{"Thread":{"List":null}}}),
    );
    let response = recv(&mut next);
    assert_eq!(response["kind"], "result", "{response}");
    let calls = fixture.requests.lock().unwrap();
    assert_eq!(calls.iter().filter(|(set, _, _)| *set == 100).count(), 1);
}

#[test]
fn help_and_version_are_plain_text_without_a_session() {
    for args in [
        vec!["--help"],
        vec!["help"],
        vec!["frame", "get", "--help"],
        vec!["help", "frame", "get"],
        vec!["--version"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_yaya"))
            .args(&args)
            .env("YAYA_SESSION", "invalid")
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        if args == ["--version"] {
            assert_eq!(text, format!("yaya {}\n", env!("CARGO_PKG_VERSION")));
        } else {
            assert!(text.contains("Usage:"), "{text}");
        }
        assert!(!text.lines().any(|line| line.starts_with('{')), "{text}");
    }
}

#[test]
fn missing_subcommand_help_is_plain_text() {
    for args in [
        vec![],
        vec!["class"],
        vec!["frame"],
        vec!["break"],
        vec!["object"],
        vec!["jdwp"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_yaya"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let text = format!(
            "{}{}",
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap()
        );
        assert!(text.contains("Usage:"), "{args:?}: {text}");
        assert!(!text.lines().any(|line| line.starts_with('{')), "{text}");
    }
}

#[test]
fn interactive_help_continues_to_the_next_command() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args([
            "attach",
            "--port",
            &fixture.port.to_string(),
            "--interactive",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"help frame get\n--help\n--version\naaa bbb\nattach -p 8701\nsessions\ndaemon --id test --port 8701\nthread list -s s_abc123\nclass find Test --session s_abc123\nthread list\nthread stack --thread 0x12\nthread stack -t 0x12\nexit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Usage: yaya frame get"), "{stderr}");
    assert_eq!(
        stderr.matches("unrecognized subcommand").count(),
        4,
        "{stderr}"
    );
    assert_eq!(stderr.matches("unexpected argument").count(), 2, "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|line| line == format!("yaya {}", env!("CARGO_PKG_VERSION")))
    );
    let rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        rows.len(),
        10,
        "help/version must not append JSONL records: {text}"
    );
    assert!(rows.iter().any(|row| row["data"]["type"] == "threads"));
    let stacks: Vec<_> = rows
        .iter()
        .filter(|row| row["data"]["type"] == "frames")
        .collect();
    assert_eq!(stacks.len(), 2);
    assert_eq!(stacks[0]["data"], stacks[1]["data"]);
    assert_eq!(stacks[0]["data"]["frames"][0]["thread"], "0x12");
    assert_eq!(stacks[0]["data"]["frames"][0]["frame"], "0x22");
    assert!(rows.iter().any(|row| row["data"]["operation"] == "dispose"));
}

#[test]
fn interactive_uses_same_commands_and_jsonl() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args([
            "attach",
            "--port",
            &fixture.port.to_string(),
            "--interactive",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"events\nthread list\nthread step --thread 0x12 --into --out\nclass find 'oops\nthread list\ndetach\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot be used with"), "{stderr}");
    assert!(stderr.contains("unclosed quote or escape"), "{stderr}");
    assert!(!rows.iter().any(|row| row["kind"] == "error"));
    assert_eq!(
        rows.iter()
            .filter(|row| row["data"]["type"] == "threads")
            .count(),
        2
    );
    assert!(rows.iter().any(|row| row["data"]["operation"] == "dispose"));
    assert!(!fixture.temp.join("yaya").exists());
}

#[test]
fn direct_field_write_does_not_read_or_suspend() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let (output, records) = fixture.run(&[
        "object",
        "--session",
        &id,
        "set",
        "0x123",
        "height",
        "--type",
        "int",
        "--value",
        "85",
    ]);
    assert!(output.status.success(), "{records:?}");
    let calls = fixture.requests.lock().unwrap();
    assert!(
        !calls
            .iter()
            .any(|(set, command, _)| matches!((*set, *command), (9, 2) | (1, 8) | (11, 2)))
    );
    let payload = &calls
        .iter()
        .find(|(set, command, _)| (*set, *command) == (9, 3))
        .unwrap()
        .2;
    assert_eq!(&payload[payload.len() - 4..], &85u32.to_be_bytes());
    drop(calls);
    let (output, records) = fixture.run(&[
        "object",
        "--session",
        &id,
        "inspect",
        "0x123",
        "--fields",
        "height",
    ]);
    assert!(output.status.success(), "{records:?}");
    assert_eq!(records[0]["data"]["fields"][0]["value"]["value"], "85");
}

#[test]
fn disconnect_removes_socket_and_does_not_reconnect() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let path = fixture.temp.join("yaya").join(&id);
    fixture
        .target
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .shutdown(Shutdown::Both)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while path.exists() {
        assert!(
            Instant::now() < deadline,
            "daemon did not clean its directory"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let (output, records) = fixture.run(&["status", "--session", &id]);
    assert!(!output.status.success());
    assert_eq!(records[0]["error"]["code"], "session_unavailable");
}

#[test]
fn session_argument_overrides_environment_and_wait_timeout_is_success() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let output = fixture
        .command()
        .env("YAYA_SESSION", "invalid")
        .args(["events", "--session", &id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let output = fixture
        .command()
        .env("YAYA_SESSION", &id)
        .args(["events", "--wait", "1s"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(rows.last().unwrap()["reason"], "timeout");
}

#[test]
fn clap_errors_use_stderr_and_exit_code_two() {
    for args in [
        vec!["--unknown"],
        vec!["attach"],
        vec!["thread", "step", "--thread", "1", "--into", "--out"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_yaya"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("error:"), "{stderr}");
        assert!(!stderr.lines().any(|line| line.starts_with('{')));
    }
}

#[test]
fn commands_reject_missing_and_extra_values() {
    for count in [0, 2] {
        let fixture = Fixture::with_value_count(count);
        let id = fixture.attach();
        for (args, message) in [
            (
                vec![
                    "frame",
                    "get",
                    "0x22",
                    "-t",
                    "0x12",
                    "--registers",
                    "v0:int",
                    "-s",
                    &id,
                ],
                "register reply count mismatch",
            ),
            (
                vec!["object", "inspect", "0x33", "--fields", "height", "-s", &id],
                "field reply count mismatch",
            ),
        ] {
            let (output, records) = fixture.run(&args);
            assert!(!output.status.success());
            assert!(
                serde_json::to_string(&records).unwrap().contains(message),
                "{records:?}"
            );
        }
        let (output, _) = fixture.run(&["detach", "-s", &id]);
        assert!(output.status.success());
    }
}

#[test]
fn daemon_survives_launching_shell_hangup() {
    let fixture = Fixture::new();
    let output = Command::new("/bin/sh")
        .args([
            "-c",
            "\"$1\" attach -p \"$2\"; kill -HUP 0",
            "yaya-test",
            env!("CARGO_BIN_EXE_yaya"),
            &fixture.port.to_string(),
        ])
        .env("TMPDIR", &fixture.temp)
        .env_remove("YAYA_SESSION")
        .process_group(0)
        .output()
        .unwrap();
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let id = records[0]["session"].as_str().unwrap();
    let (output, records) = fixture.run(&["status", "-s", id]);
    assert!(output.status.success(), "{records:?}");
    let (output, records) = fixture.run(&["detach", "-s", id]);
    assert!(output.status.success(), "{records:?}");
}

#[test]
fn attach_failure_reports_error_and_removes_session() {
    let fixture = Fixture::new();
    let (output, records) = fixture.run(&["attach", "--host", "invalid host", "-p", "1"]);
    assert!(!output.status.success());
    assert_eq!(records[0]["kind"], "error");
    assert!(
        fs::read_dir(fixture.temp.join("yaya"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        !fixture
            .command()
            .args(["daemon", "--help"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn daemon_cleans_up_when_attach_dies_during_startup() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let temp = PathBuf::from("/tmp").join(format!(
        "yy-cancel-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&temp).unwrap();
    let mut attach = Command::new(env!("CARGO_BIN_EXE_yaya"))
        .args([
            "attach",
            "-p",
            &listener.local_addr().unwrap().port().to_string(),
        ])
        .env("TMPDIR", &temp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    let (mut target, _) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "daemon did not connect");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    };
    target.set_nonblocking(false).unwrap();
    target
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    let mut handshake = [0; 14];
    target.read_exact(&mut handshake).unwrap();
    assert_eq!(&handshake, b"JDWP-Handshake");
    attach.kill().unwrap();
    attach.wait().unwrap();
    target.write_all(&handshake).unwrap();
    let mut header = [0; 11];
    target.read_exact(&mut header).unwrap();
    let request_id = u32::from_be_bytes(header[4..8].try_into().unwrap());
    assert_eq!(&header[9..], &[1, 7]);
    let sizes: Vec<u8> = [8u32; 5].into_iter().flat_map(u32::to_be_bytes).collect();
    target
        .write_all(&packet(request_id, 0x80, 0, 0, &sizes))
        .unwrap();
    target.read_exact(&mut header).unwrap();
    assert_eq!(&header[9..], &[1, 6]);
    let request_id = u32::from_be_bytes(header[4..8].try_into().unwrap());
    target
        .write_all(&packet(request_id, 0x80, 0, 0, &[]))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while fs::read_dir(temp.join("yaya")).unwrap().next().is_some() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(fs::read_dir(temp.join("yaya")).unwrap().next().is_none());
    fs::remove_dir(temp.join("yaya")).unwrap();
    fs::remove_dir(temp).unwrap();
}

#[test]
fn events_require_explicit_subscription_in_both_modes() {
    for interactive in [false, true] {
        let fixture = Fixture::new();
        let groups: Vec<Vec<Value>> = if interactive {
            let mut child = fixture
                .command()
                .args(["attach", "-p", &fixture.port.to_string(), "--interactive"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"thread list\nstatus\nevents\nevents -w 1s\nstatus\nexit\n")
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            let mut groups = Vec::new();
            let mut group = Vec::new();
            for line in String::from_utf8(output.stdout).unwrap().lines() {
                let record: Value = serde_json::from_str(line).unwrap();
                let end = record["kind"] == "end";
                group.push(record);
                if end {
                    groups.push(std::mem::take(&mut group));
                }
            }
            assert_eq!(groups.len(), 7, "{groups:?}");
            groups.remove(0);
            groups
        } else {
            let id = fixture.attach();
            [
                vec!["thread", "list"],
                vec!["status"],
                vec!["events"],
                vec!["events", "-w", "1s"],
                vec!["status"],
                vec!["detach"],
            ]
            .into_iter()
            .map(|mut args| {
                args.extend(["-s", id.as_str()]);
                let (output, records) = fixture.run(&args);
                assert!(output.status.success(), "{records:?}");
                records
            })
            .collect()
        };
        assert!(
            groups[..2]
                .iter()
                .flatten()
                .all(|record| record["kind"] != "event_batch")
        );
        assert!(groups[1][0]["data"]["queued_wire_bytes"].as_u64().unwrap() > 0);
        assert_eq!(
            groups[2]
                .iter()
                .filter(|record| record["kind"] == "event_batch")
                .count(),
            1
        );
        assert_eq!(groups[3].last().unwrap()["reason"], "timeout");
        assert_eq!(groups[4][0]["data"]["queued_wire_bytes"], 0);
    }
}

#[test]
fn interactive_eof_finishes_only_active_commands() {
    for input in ["", "events --collect 30s\n"] {
        let fixture = Fixture::new();
        let mut child = fixture
            .command()
            .args(["attach", "-p", &fixture.port.to_string(), "--interactive"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let ends: Vec<_> = rows.iter().filter(|row| row["kind"] == "end").collect();
        assert_eq!(ends.len(), if input.is_empty() { 1 } else { 2 }, "{rows:?}");
        assert!(ends.iter().all(|row| row["ok"] == true));
    }
}

#[test]
fn interactive_disconnect_during_wait_is_reported_once() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args(["attach", "-p", &fixture.port.to_string(), "-i"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let read_end = |reader: &mut BufReader<ChildStdout>| {
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if serde_json::from_str::<Value>(&line).unwrap()["kind"] == "end" {
                break;
            }
        }
    };
    read_end(&mut reader);
    input.write_all(b"events\n").unwrap();
    input.flush().unwrap();
    read_end(&mut reader);
    input.write_all(b"events -w 30s\n").unwrap();
    input.flush().unwrap();
    thread::sleep(Duration::from_millis(100));
    fixture
        .target
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .shutdown(Shutdown::Both)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("interactive did not exit after disconnect");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();
    let rows: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(status.code(), Some(1));
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["error"]["code"], "disconnected");
    assert_eq!(rows[1]["kind"], "end");
    assert_eq!(rows[1]["ok"], false);
}

#[test]
fn ddm_commands_share_socket_and_channel_paths() {
    for interactive in [false, true] {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.temp).unwrap();
        let commands = [
            vec!["ddm", "features"],
            vec!["ddm", "raw", "VULW"],
            vec!["ddm", "raw", "NONE"],
            vec!["ddm", "raw", "FAIL"],
            vec!["ddm", "raw", "JDWE"],
            vec![
                "ui", "capture", "-w", "window", "-v", "bad", "-o", "bad.png",
            ],
            vec!["ui", "windows"],
            vec!["ui", "dump", "-w", "window"],
            vec![
                "ui",
                "capture",
                "-w",
                "window",
                "-v",
                "android.widget.FrameLayout@123",
                "-o",
                "capture.png",
            ],
        ];
        let records = if interactive {
            let mut child = fixture
                .command()
                .current_dir(&fixture.temp)
                .args(["attach", "-p", &fixture.port.to_string(), "-i"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            for command in &commands {
                writeln!(stdin, "{}", command.join(" ")).unwrap();
            }
            writeln!(stdin, "exit").unwrap();
            drop(stdin);
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        } else {
            let session = fixture.attach();
            let mut records = Vec::new();
            for command in &commands {
                let output = fixture
                    .command()
                    .current_dir(&fixture.temp)
                    .args(command)
                    .args(["-s", &session])
                    .output()
                    .unwrap();
                records.extend(
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str::<Value>(line).unwrap()),
                );
            }
            assert!(fixture.run(&["detach", "-s", &session]).0.status.success());
            records
        };
        assert!(
            records
                .iter()
                .any(|row| row["data"]["features"][0] == "view-hierarchy")
        );
        assert!(records.iter().any(|row| row["data"]["chunks"] == json!([])));
        assert!(
            records
                .iter()
                .any(|row| row["error"]["ddm_error_code"] == -2)
        );
        assert!(records.iter().any(|row| row["error"]["code"] == "jdwp_error" && row["error"]["error_code"] == 99));
        assert!(
            records
                .iter()
                .any(|row| row["data"]["hierarchy"]["nodes"][1]["parent"] == 0)
        );
        assert!(records.iter().any(|row| {
            row["data"]["path"]
                == fixture
                    .temp
                    .canonicalize()
                    .unwrap()
                    .join("capture.png")
                    .to_str()
                    .unwrap()
        }));
        assert!(
            !records
                .iter()
                .any(|row| row["data"]["type"] == "view_capture")
        );
        assert_eq!(
            fs::read(fixture.temp.join("capture.png")).unwrap(),
            include_bytes!("fixtures/capture.png")
        );
        assert!(!fixture.temp.join("bad.png").exists());
        assert!(records.iter().any(|row| {
            row["error"]["message"]
                .as_str()
                .is_some_and(|text| text.contains("did not return a PNG"))
        }));
        assert!(
            records
                .iter()
                .any(|row| row["data"]["type"] == "file_saved")
        );
        let requests = fixture.requests.lock().unwrap();
        let capture = requests
            .iter()
            .find(|(set, _, body)| *set == 199 && body.starts_with(b"VUOP"))
            .unwrap();
        assert!(!capture.2.windows(11).any(|bytes| bytes == b"capture.png"));
    }
}

#[test]
fn primitive_inspection_works_over_socket_and_channel() {
    for interactive in [false, true] {
        let fixture = Fixture::new();
        let commands = [
            "thread suspend-count -t 0x12",
            "thread suspend -t 0x12",
            "vm suspend",
            "thread suspend-count -t 0x12",
            "frame this 0x22 -t 0x12",
            "string get 0x55",
            "object referrers 0x55 --limit 0",
            "array length 0x66",
            "array get 0x66 0:3",
            "array set 0x66 1:3 --type int --values=-7,42",
            "array get 0x66 0:3",
            "array get 0x66 3:3",
            "array get 0x66 4:5",
            "array set 0x66 0:1 --type byte --values=256",
            "array set 0x66 0:2 --type int --values=1",
            "thread resume -t 0x12",
            "thread suspend-count -t 0x12",
            "vm resume",
            "thread suspend-count -t 0x12",
        ];
        let records = fixture.run_commands(interactive, &commands);
        let counts: Vec<_> = records
            .iter()
            .filter(|row| row["data"]["type"] == "suspend_count")
            .map(|row| row["data"]["count"].as_u64().unwrap())
            .collect();
        assert_eq!(counts, [0, 2, 1, 0]);
        assert!(
            records
                .iter()
                .any(|row| row["data"]["type"] == "this_object"
                    && row["data"]["object"]["value"] == "0x55")
        );
        assert!(
            records
                .iter()
                .any(|row| row["data"]["value"] == "hello 你好")
        );
        assert!(records.iter().any(|row| row["data"]["type"] == "referrers"
            && row["data"]["objects"][0]["value"] == "0x88"));
        assert!(records.iter().any(|row| row["data"]["length"] == 3));
        assert!(
            records
                .iter()
                .any(|row| row["data"]["values"][1]["value"] == "-7"
                    && row["data"]["values"][2]["value"] == "42")
        );
        assert!(records.iter().any(|row| row["data"]["values"] == json!([])));
        assert_eq!(
            records.iter().filter(|row| row["kind"] == "error").count(),
            3,
            "{records:?}"
        );
        let requests = fixture.requests.lock().unwrap();
        let writes: Vec<_> = requests
            .iter()
            .filter(|(set, command, _)| (*set, *command) == (13, 3))
            .collect();
        assert_eq!(writes.len(), 1, "invalid byte must not be sent");
        assert_eq!(&writes[0].2[16..], &[255, 255, 255, 249, 0, 0, 0, 42]);
        let position = requests
            .iter()
            .position(|(set, command, _)| (*set, *command) == (13, 3))
            .unwrap();
        assert_eq!(
            (requests[position - 1].0, requests[position - 1].1),
            (13, 2),
            "no hidden preflight before array write"
        );
    }
}

#[test]
fn watches_share_socket_and_channel_registration_delivery_and_clear() {
    for interactive in [false, true] {
        let fixture = Fixture::new();
        let commands = [
            "exception watch --all",
            "exception watch --all --with-caught --suspend none -t 0x12 --count 2",
            "exception watch Ltest; --type-id 0x101 --caught-only",
            "exception unwatch 77",
            "class prepare watch test.* --count 1",
            "class prepare unwatch 77",
            "field access watch Ltest; height --type-id 0x101 -t 0x12",
            "field access unwatch 77",
            "field modification watch Ltest; height --suspend all",
            "field modification unwatch 77",
            "field access watch Ltest; missing",
            "events",
        ];
        let rows = fixture.run_commands(interactive, &commands);
        assert_eq!(
            rows.iter().filter(|row| row["kind"] == "error").count(),
            1,
            "{rows:?}"
        );
        let kinds: Vec<_> = rows
            .iter()
            .filter_map(|row| row["batch"]["events"].as_array())
            .flatten()
            .filter_map(|event| event["kind"].as_str())
            .collect();
        for expected in [
            "exception",
            "class_prepared",
            "field_access",
            "field_modification",
        ] {
            assert!(kinds.contains(&expected), "{kinds:?}");
        }
        let requests = fixture.requests.lock().unwrap();
        let sets: Vec<_> = requests
            .iter()
            .filter(|(set, command, _)| (*set, *command) == (15, 1))
            .map(|(_, _, body)| body)
            .collect();
        assert_eq!(sets.len(), 6);
        assert_eq!(&sets[0][..7], &[4, 1, 0, 0, 0, 1, 8]);
        assert_eq!(&sets[0][7..], &[0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(sets[1][1], 0);
        assert_eq!(&sets[1][15..17], &[1, 1]);
        assert_eq!(
            &sets[1][17..],
            &[3, 0, 0, 0, 0, 0, 0, 0, 0x12, 1, 0, 0, 0, 2]
        );
        assert_eq!(&sets[2][7..15], &0x101u64.to_be_bytes());
        assert_eq!(&sets[2][15..], &[1, 0]);
        assert_eq!(sets[3][0], 8);
        assert_eq!(sets[3][6], 5);
        assert_eq!(&sets[3][11..17], b"test.*");
        assert_eq!(&sets[3][17..], &[1, 0, 0, 0, 1]);
        for (body, kind) in [(sets[4], 20), (sets[5], 21)] {
            assert_eq!(body[0], kind);
            assert_eq!(body[6], 9);
            assert_eq!(&body[7..15], &0x101u64.to_be_bytes());
            assert_eq!(&body[15..23], &0x45u64.to_be_bytes());
        }
        let cleared: Vec<_> = requests
            .iter()
            .filter(|(set, command, _)| (*set, *command) == (15, 2))
            .map(|(_, _, body)| body.clone())
            .collect();
        assert_eq!(cleared, [4, 8, 20, 21].map(|kind| vec![kind, 0, 0, 0, 77]));
        // ClassPrepare itself must not look up already-loaded types.
        let position = requests
            .iter()
            .position(|(set, command, body)| (*set, *command) == (15, 1) && body[0] == 8)
            .unwrap();
        assert_eq!(
            (requests[position - 1].0, requests[position - 1].1),
            (15, 2)
        );
        assert!(!requests.iter().any(|(set, command, _)| matches!(
            (*set, *command),
            (9, 2) | (1, 8) | (11, 2) | (1, 9) | (11, 3)
        )));
    }
}
