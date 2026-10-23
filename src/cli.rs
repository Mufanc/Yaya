use clap::{Args as ClapArgs, Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::wire;

macro_rules! help_text {
    ($command:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/help/",
            $command,
            ".txt"
        ))
    };
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Inspect and control a running Android app through JDWP",
    after_help = help_text!("yaya")
)]
pub struct Args {
    #[command(subcommand)]
    pub command: EntryCommand,
}

#[derive(ClapArgs, Clone, Debug, Default)]
pub struct SessionArgs {
    /// Target session ID; overrides YAYA_SESSION; interactive uses its current session.
    #[arg(short = 's', long, global = true, value_name = "ID")]
    pub session: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum EntryCommand {
    /// Connect to an existing JDWP endpoint and create a session.
    #[command(after_help = help_text!("attach"))]
    Attach {
        /// Host of an already available JDWP TCP endpoint.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// JDWP TCP port (1..65535); not an ADB server port.
        #[arg(short = 'p', long, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
        /// Keep the session in a foreground command loop instead of starting a daemon.
        #[arg(short = 'i', long)]
        interactive: bool,
    },
    /// List session-directory IDs without checking whether they are alive.
    #[command(after_help = help_text!("sessions"))]
    Sessions,
    #[command(flatten)]
    SessionCommand(Command),
}

#[derive(ClapArgs, Clone, Debug, Default)]
pub struct CurrentSession {}

#[derive(Parser, Debug)]
#[command(name = "yaya", version, about = "Debug the current attached session", after_help = help_text!("interactive"))]
pub struct InteractiveArgs {
    #[command(subcommand)]
    pub command: InteractiveCommand,
}

#[derive(Subcommand, Debug)]
pub enum InteractiveCommand {
    #[command(flatten)]
    Debug(Command<CurrentSession>),
    /// Detach and leave interactive mode (interactive only).
    #[command(after_help = help_text!("exit"))]
    Exit,
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Command<S: ClapArgs = SessionArgs> {
    /// Show the endpoint, ID sizes and buffered-event usage.
    #[command(after_help = help_text!("status"))]
    Status {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
    },
    /// Dispose the debug connection and end the session without killing the app.
    #[command(after_help = help_text!("detach"))]
    Detach {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
    },
    /// Find loaded classes and inspect methods or fields.
    #[command(after_help = help_text!("class/index"))]
    Class {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Class,
    },
    /// Find live objects and read or write selected fields.
    #[command(after_help = help_text!("object/index"))]
    Object {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Object,
    },
    /// Add or remove DEX-location breakpoints.
    #[command(after_help = help_text!("break/index"))]
    Break {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Break,
    },
    /// List thread IDs, names, execution status and suspend status.
    #[command(after_help = help_text!("threads"))]
    Threads {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
    },
    /// Read a suspended thread's full call stack with thread and frame IDs.
    #[command(after_help = help_text!("stack"))]
    Stack {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        /// Thread ID from threads or an event (decimal or 0x).
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
    },
    /// Read or write registers in a suspended stack frame.
    #[command(after_help = help_text!("frame/index"))]
    Frame {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Frame,
    },
    /// Register one instruction step and resume the thread; does not wait.
    #[command(after_help = help_text!("step"))]
    #[command(group(clap::ArgGroup::new("depth").required(true).args(["into", "over", "out"])))]
    Step {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        /// Thread ID from threads or an event (decimal or 0x).
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
        /// Step into a method call.
        #[arg(long)]
        into: bool,
        /// Step over a method call.
        #[arg(long)]
        over: bool,
        /// Run until this method returns to its caller.
        #[arg(long)]
        out: bool,
    },
    /// Send one thread or VM resume request.
    #[command(after_help = help_text!("resume"))]
    #[command(group(clap::ArgGroup::new("target").required(true).args(["thread", "all"])))]
    Resume {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        /// Thread ID from threads or an event (decimal or 0x).
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: Option<u64>,
        /// Send one VM-wide Resume request.
        #[arg(short = 'a', long)]
        all: bool,
    },
    /// Drain buffered events, wait for a batch, or collect for a duration.
    #[command(after_help = help_text!("events"))]
    Events {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        /// Wait at most DURATION for the first event batch, e.g. 30s or 2m.
        #[arg(short = 'w', long, conflicts_with = "collect", value_parser = duration)]
        wait: Option<u64>,
        /// Collect for DURATION, returning early if a batch suspends execution.
        #[arg(short = 'c', long, value_parser = duration)]
        collect: Option<u64>,
    },
    /// Inspect ID widths or send a raw JDWP request.
    #[command(after_help = help_text!("jdwp/index"))]
    Jdwp {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Jdwp,
    },
}

impl Command {
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Status { session, .. }
            | Self::Detach { session, .. }
            | Self::Threads { session, .. }
            | Self::Class { session, .. }
            | Self::Object { session, .. }
            | Self::Break { session, .. }
            | Self::Frame { session, .. }
            | Self::Jdwp { session, .. }
            | Self::Stack { session, .. }
            | Self::Step { session, .. }
            | Self::Resume { session, .. }
            | Self::Events { session, .. } => session.session.as_deref(),
        }
    }
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Class {
    /// Find loaded classes by case-sensitive JNI-signature substring.
    #[command(after_help = help_text!("class/find"))]
    Find {
        /// Literal substring of a loaded JNI signature, e.g. com/example.
        pattern: String,
    },
    /// List methods declared by a loaded class.
    #[command(after_help = help_text!("class/methods"))]
    Methods {
        /// Exact loaded JNI signature, e.g. Lcom/example/Card;
        class: String,
        /// Runtime reference-type ID to resolve class-loader ambiguity (decimal or 0x).
        #[arg(long, value_parser = wire::number)]
        type_id: Option<u64>,
    },
    /// List fields, including inherited class and interface fields.
    #[command(after_help = help_text!("class/fields"))]
    Fields {
        /// Exact loaded JNI signature, e.g. Lcom/example/Card;
        class: String,
        /// Runtime reference-type ID to resolve class-loader ambiguity (decimal or 0x).
        #[arg(long, value_parser = wire::number)]
        type_id: Option<u64>,
    },
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Object {
    /// List up to LIMIT live instances of a loaded class.
    #[command(after_help = help_text!("object/instances"))]
    Instances {
        /// Exact loaded JNI signature, e.g. Lcom/example/Card;
        class: String,
        /// Runtime reference-type ID to resolve class-loader ambiguity (decimal or 0x).
        #[arg(long, value_parser = wire::number)]
        type_id: Option<u64>,
        /// Maximum instances to return; required, from 1 through 2147483647.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=2147483647))]
        limit: u32,
    },
    /// Read explicitly selected instance or static fields.
    #[command(after_help = help_text!("object/inspect"))]
    Inspect {
        /// Runtime object ID (decimal or 0x).
        #[arg(value_parser = wire::number)]
        object: u64,
        /// Comma-separated field names or declaring-type-id:name selectors.
        #[arg(long, value_delimiter = ',', required = true)]
        fields: Vec<String>,
    },
    /// Write one field directly, without reading its old value or suspending.
    #[command(after_help = help_text!("object/set"))]
    Set {
        /// Runtime object ID (decimal or 0x).
        #[arg(value_parser = wire::number)]
        object: u64,
        /// Field name, or declaring-type-id:name when hidden by another declaration.
        field: String,
        /// Value type: boolean, byte, char, short, int, long, float, double, object.
        #[arg(long = "type", value_parser = wire::value_tag)]
        tag: u8,
        /// Typed value; decimal integers, true/false, or object ID/null.
        #[arg(long, allow_hyphen_values = true)]
        value: String,
    },
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Break {
    /// Set a breakpoint at an exact method and DEX code-unit offset.
    #[command(after_help = help_text!("break/add"))]
    Add {
        /// Full method signature, e.g. Lcom/example/Card;->compare()Z.
        signature: String,
        /// DEX offset in 16-bit code units (decimal or 0x), not bytes.
        #[arg(long, value_parser = wire::number)]
        pc: u64,
        /// Runtime reference-type ID to resolve class-loader ambiguity (decimal or 0x).
        #[arg(long, value_parser = wire::number)]
        type_id: Option<u64>,
        /// Which threads to suspend on a breakpoint hit.
        #[arg(long, default_value = "thread", value_parser = ["none", "thread", "all"])]
        suspend: String,
    },
    /// Remove one breakpoint request; does not resume paused threads.
    #[command(after_help = help_text!("break/remove"))]
    Remove {
        /// Decimal event request ID returned by break add.
        id: u32,
    },
    /// Clear all breakpoints on this connection, including raw-created ones.
    #[command(after_help = help_text!("break/clear"))]
    Clear,
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Frame {
    /// Read explicitly typed DEX registers or JDWP slots.
    #[command(after_help = help_text!("frame/get"))]
    Get {
        /// Frame ID returned by stack (decimal or 0x); refresh after resume/step.
        #[arg(value_parser = wire::number)]
        frame: u64,
        /// Thread ID belonging to this frame (decimal or 0x).
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
        /// Comma-separated register:type pairs, e.g. v0:int,v2:object or slot2:object.
        #[arg(long, value_delimiter = ',', required = true)]
        registers: Vec<String>,
    },
    /// Write one typed register or slot directly; no read-back or undo.
    #[command(after_help = help_text!("frame/set"))]
    Set {
        /// Frame ID returned by stack (decimal or 0x); refresh after resume/step.
        #[arg(value_parser = wire::number)]
        frame: u64,
        /// Thread ID belonging to this frame (decimal or 0x).
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
        /// DEX register vN or explicit JDWP slotN (modern ART direct numbering).
        register: String,
        /// Value type: boolean, byte, char, short, int, long, float, double, object.
        #[arg(long = "type", value_parser = wire::value_tag)]
        tag: u8,
        /// Typed value; decimal integers, true/false, or object ID/null.
        #[arg(long, allow_hyphen_values = true)]
        value: String,
    },
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Jdwp {
    /// Show the ID widths negotiated when this session attached.
    #[command(after_help = help_text!("jdwp/idsizes"))]
    Idsizes,
    /// Send a headerless JDWP payload and return the raw reply.
    #[command(after_help = help_text!("jdwp/raw"))]
    Raw {
        /// JDWP command-set:command; each number is decimal or 0x, in 0..255.
        #[arg(value_name = "COMMAND-SET:COMMAND", value_parser = command_address)]
        #[serde(flatten)]
        address: CommandAddress,
        /// Hex-encoded payload bytes; omit for an empty payload.
        #[arg(value_name = "HEX", default_value = "")]
        payload_hex: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandAddress {
    pub command_set: u8,
    pub command: u8,
}

fn command_address(value: &str) -> Result<CommandAddress, String> {
    let (command_set, command) = value
        .split_once(':')
        .ok_or("expected command-set:command")?;
    let parse_byte = |part| {
        u8::try_from(wire::number(part)?)
            .map_err(|_| "command-set and command must be in 0..255".to_owned())
    };

    Ok(CommandAddress {
        command_set: parse_byte(command_set)?,
        command: parse_byte(command)?,
    })
}

pub fn duration(value: &str) -> Result<u64, String> {
    let (digits, multiplier) = if let Some(s) = value.strip_suffix('s') {
        (s, 1)
    } else if let Some(s) = value.strip_suffix('m') {
        (s, 60)
    } else {
        return Err("duration requires s or m".into());
    };

    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err("duration requires a positive integer".into());
    }

    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| (1..=86400).contains(n))
        .ok_or_else(|| "duration must be 1 second through 24 hours".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_examples_match_the_current_parser() {
        let mut directories =
            vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("help")];
        let mut checked = 0;
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    directories.push(path);
                    continue;
                }
                for line in std::fs::read_to_string(&path).unwrap().lines() {
                    let line = line.trim();
                    if line.starts_with("yaya ") {
                        let words = shlex::split(line).unwrap();
                        assert!(
                            Args::try_parse_from(words).is_ok(),
                            "{}: {line}",
                            path.display()
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked > 30,
            "expected external CLI examples throughout help"
        );
    }

    #[test]
    fn raw_address_and_optional_payload() {
        for (address, payload) in [("16:3", "AA55aa55"), ("0x10:0x3", "")] {
            let args = Args::try_parse_from(["yaya", "jdwp", "raw", address, payload]).unwrap();
            let EntryCommand::SessionCommand(Command::Jdwp { command, .. }) = args.command else {
                panic!("expected jdwp command")
            };
            assert_eq!(
                serde_json::to_value(command).unwrap(),
                serde_json::json!({
                    "Raw": {"command_set": 16, "command": 3, "payload_hex": payload}
                })
            );
        }
        let args = Args::try_parse_from(["yaya", "jdwp", "raw", "1:1"]).unwrap();
        let EntryCommand::SessionCommand(Command::Jdwp { command, .. }) = args.command else {
            panic!("expected jdwp command")
        };
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            serde_json::json!({
                "Raw": {"command_set": 1, "command": 1, "payload_hex": ""}
            })
        );
        assert!(InteractiveArgs::try_parse_from(["yaya", "jdwp", "raw", "1:1"]).is_ok());
        for address in ["16", ":3", "16:", "16:3:4", "256:1", "1:256", "-1:3", "x:3"] {
            assert!(Args::try_parse_from(["yaya", "jdwp", "raw", address]).is_err());
        }
    }

    #[test]
    fn frame_ids_and_threads_are_explicit_numbers() {
        for args in [
            vec!["frame", "get", "0x22", "-t", "18", "--registers", "v0:int"],
            vec![
                "frame", "set", "34", "--thread", "0x12", "v0", "--type", "int", "--value", "42",
            ],
        ] {
            assert!(InteractiveArgs::try_parse_from(std::iter::once("yaya").chain(args)).is_ok());
        }
        for args in [
            vec!["frame", "get", "0x22", "--registers", "v0:int"],
            vec![
                "frame",
                "get",
                "s_abc123:0x12:0x22",
                "-t",
                "0x12",
                "--registers",
                "v0:int",
            ],
        ] {
            assert!(InteractiveArgs::try_parse_from(std::iter::once("yaya").chain(args)).is_err());
        }
    }

    #[test]
    fn interactive_has_only_current_session_commands() {
        for args in [
            vec!["attach", "-p", "8701"],
            vec!["sessions"],
            vec!["daemon"],
            vec!["threads", "-s", "s_abc123"],
            vec!["class", "find", "Card", "--session", "s_abc123"],
        ] {
            assert!(InteractiveArgs::try_parse_from(std::iter::once("yaya").chain(args)).is_err());
        }
        assert!(InteractiveArgs::try_parse_from(["yaya", "threads"]).is_ok());
        assert!(InteractiveArgs::try_parse_from(["yaya", "exit"]).is_ok());
        assert!(Args::try_parse_from(["yaya", "exit"]).is_err());

        fn check_help(command: &mut clap::Command) {
            assert!(!command.get_arguments().any(|arg| arg.get_id() == "session"));
            let help = command.render_long_help().to_string();
            assert!(!help.contains("--session"), "{help}");
            for child in command.get_subcommands_mut() {
                check_help(child);
            }
        }

        let mut command = InteractiveArgs::command();
        command.build();
        for name in ["attach", "sessions", "daemon"] {
            assert!(command.find_subcommand(name).is_none());
        }
        check_help(&mut command);
    }

    #[test]
    fn session_is_scoped_to_existing_session_commands() {
        for args in [
            vec!["threads", "-s", "s_abc123"],
            vec!["class", "-s", "s_abc123", "find", "Card"],
            vec!["class", "find", "Card", "--session", "s_abc123"],
            vec![
                "object", "inspect", "1", "--fields", "height", "-s", "s_abc123",
            ],
            vec!["break", "clear", "-s", "s_abc123"],
            vec![
                "frame",
                "get",
                "0x22",
                "-t",
                "0x12",
                "--registers",
                "v0:int",
                "-s",
                "s_abc123",
            ],
            vec!["jdwp", "idsizes", "-s", "s_abc123"],
        ] {
            let args = Args::try_parse_from(std::iter::once("yaya").chain(args)).unwrap();
            let EntryCommand::SessionCommand(command) = args.command else {
                panic!("expected debug command")
            };
            assert_eq!(command.session(), Some("s_abc123"));
        }
        for args in [
            vec!["-s", "s_abc123", "threads"],
            vec!["attach", "-p", "8701", "-s", "s_abc123"],
            vec![
                "attach",
                "-p",
                "8701",
                "--interactive",
                "--session",
                "s_abc123",
            ],
            vec!["sessions", "-s", "s_abc123"],
        ] {
            let error = Args::try_parse_from(std::iter::once("yaya").chain(args)).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
        for args in [
            vec!["--help"],
            vec!["attach", "--help"],
            vec!["sessions", "--help"],
        ] {
            let help = Args::try_parse_from(std::iter::once("yaya").chain(args)).unwrap_err();
            assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
            assert!(!help.to_string().contains("--session <ID>"));
        }
        let args = Args::try_parse_from(["yaya", "attach", "-p", "8701"]).unwrap();
        assert!(matches!(
            args.command,
            EntryCommand::Attach { port: 8701, .. }
        ));
    }

    #[test]
    fn validates_exclusive_modes() {
        assert!(
            Args::try_parse_from(["yaya", "events", "--wait", "2s", "--collect", "3s"]).is_err()
        );
        assert!(Args::try_parse_from(["yaya", "step", "--thread", "1"]).is_err());
        for pair in [
            ["--into", "--over"],
            ["--into", "--out"],
            ["--over", "--out"],
        ] {
            assert!(
                Args::try_parse_from(["yaya", "step", "--thread", "1", pair[0], pair[1]]).is_err()
            );
        }
        assert!(Args::try_parse_from(["yaya", "resume", "--all", "--thread", "1"]).is_err());
        assert_eq!(duration("2m").unwrap(), 120);
        for invalid in ["0s", "1ms", "0.5s", "+1s", "999999999999999999999m"] {
            assert!(duration(invalid).is_err());
        }
        let words = shlex::split("break add 'Ltest;->foo()Z' --pc 0x19").unwrap();
        assert!(
            Args::try_parse_from(std::iter::once("yaya").chain(words.iter().map(String::as_str)))
                .is_ok()
        );
    }
}
