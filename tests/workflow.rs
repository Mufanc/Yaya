use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

type RecordedRequests = Arc<Mutex<Vec<(u8, u8, Vec<u8>)>>>;

struct Fixture {
    temp: std::path::PathBuf,
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
    if kind == 1 || kind == 2 {
        body.push(1);
        body.extend(0x101u64.to_be_bytes());
        body.extend(0x82u64.to_be_bytes());
        body.extend(0x19u64.to_be_bytes());
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
        let temp = std::path::PathBuf::from(format!(
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
                    (15, 1) => reply.extend(77u32.to_be_bytes()),
                    (11, 3) => {
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
                    (16, 2) | (15, 2) | (15, 3) | (1, 9) => (),
                    (1, 6) => {
                        stream.write_all(&packet(id, 0x80, 0, 0, &[])).unwrap();
                        break;
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
                json!({"kind":"hello","version":4,"session":id}),
            );
            let response = recv(&mut socket);
            if response["kind"] == "ready" {
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
    let bytes = serde_json::to_vec(&value).unwrap();
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
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn daemon_debugging_workflow() {
    let fixture = Fixture::new();
    let id = fixture.attach();
    let (output, records) = fixture.run(&["threads", "--session", &id]);
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
    let (output, records) = fixture.run(&["step", "--session", &id, "--thread", "0x12", "--over"]);
    assert!(output.status.success(), "{records:?}");
    let (output, records) = fixture.run(&["events", "--session", &id, "--wait", "2s"]);
    assert!(output.status.success(), "{records:?}");
    assert_eq!(records[0]["batch"]["events"][0]["kind"], "step_completed");
    let (_, records) = fixture.run(&["stack", "--session", &id, "-t", "0x12"]);
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
        json!({"kind":"execute","command":{"Threads":{}}}),
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
        .write_all(b"help frame get\n--help\n--version\naaa bbb\nattach -p 8701\nsessions\ndaemon --id test --port 8701\nthreads -s s_abc123\nclass find Test --session s_abc123\nthreads\nstack --thread 0x12\nstack -t 0x12\nexit\n")
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
        .write_all(b"events\nthreads\nstep --thread 0x12 --into --out\nclass find 'oops\nthreads\ndetach\n")
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
        vec!["step", "--thread", "1", "--into", "--out"],
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
    let temp = std::path::PathBuf::from("/tmp").join(format!(
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
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
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
                .write_all(b"threads\nstatus\nevents\nevents -w 1s\nstatus\nexit\n")
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
                vec!["threads"],
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
    let read_end = |reader: &mut BufReader<std::process::ChildStdout>| {
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
