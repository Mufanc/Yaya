use std::path::{Path, PathBuf};

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

mod watch;

pub use watch::{ClassPrepare, Exception, Field, FieldWatch, WatchOptions};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Inspect and control a running Android app through JDWP",
    after_help = help_text!("index")
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
    /// Control an existing session daemon independently of the business protocol.
    #[command(after_help = help_text!("daemon/index"))]
    Daemon {
        #[command(flatten)]
        session: SessionArgs,
        #[command(subcommand)]
        command: DaemonCommand,
    },
    #[command(flatten)]
    SessionCommand(Command),
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    /// Stop the daemon and release its debugging connection.
    #[command(after_help = help_text!("daemon/stop"))]
    Stop,
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
    /// Inspect loaded classes or watch future class preparation.
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
    /// Query array length or read and write a range of elements.
    #[command(after_help = help_text!("array/index"))]
    Array {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Array,
    },
    /// Read the contents of a VM string.
    #[command(after_help = help_text!("string/index"))]
    String {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: StringCommand,
    },
    /// Register or clear exception event requests.
    #[command(after_help = help_text!("exception/index"))]
    Exception {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Exception,
    },
    /// Register or clear field access and modification event requests.
    #[command(after_help = help_text!("field/index"))]
    Field {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Field,
    },
    /// List threads, inspect stacks and control thread execution.
    #[command(after_help = help_text!("thread/index"))]
    Thread {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Thread,
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
    /// Suspend or resume the whole VM.
    #[command(after_help = help_text!("vm/index"))]
    Vm {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Vm,
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
    /// Query Android DDM capabilities or send a raw chunk.
    #[command(after_help = help_text!("ddm/index"))]
    Ddm {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Ddm,
    },
    /// Inspect windows, View hierarchies and View images through DDM.
    #[command(after_help = help_text!("ui/index"))]
    Ui {
        #[command(flatten)]
        #[serde(skip)]
        session: S,
        #[command(subcommand)]
        #[serde(flatten)]
        command: Ui,
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

impl<S: ClapArgs> Command<S> {
    pub fn output_path(&self) -> Option<&Path> {
        match self {
            Self::Ui {
                command: Ui::Capture { output, .. },
                ..
            } => Some(output),
            _ => None,
        }
    }
}

impl Command {
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Status { session, .. }
            | Self::Detach { session, .. }
            | Self::Class { session, .. }
            | Self::Object { session, .. }
            | Self::Array { session, .. }
            | Self::String { session, .. }
            | Self::Thread { session, .. }
            | Self::Exception { session, .. }
            | Self::Field { session, .. }
            | Self::Vm { session, .. }
            | Self::Break { session, .. }
            | Self::Frame { session, .. }
            | Self::Jdwp { session, .. }
            | Self::Ddm { session, .. }
            | Self::Ui { session, .. }
            | Self::Events { session, .. } => session.session.as_deref(),
        }
    }
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Class {
    /// Watch future class preparation events.
    #[command(after_help = help_text!("class/prepare/index"))]
    Prepare {
        #[command(subcommand)]
        #[serde(flatten)]
        command: ClassPrepare,
    },
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
    /// List objects that directly reference this object.
    #[command(after_help = help_text!("object/referrers"))]
    Referrers {
        #[arg(value_parser = wire::number)]
        object: u64,
        /// Maximum referrers; 0 requests all. Does not include stack/JNI roots.
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=2147483647))]
        limit: u32,
    },
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
    /// Get the current frame's this object; static/native frames may return null.
    #[command(after_help = help_text!("frame/this"))]
    This {
        #[arg(value_parser = wire::number)]
        frame: u64,
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
    },
    /// Read explicitly typed JDWP slots.
    #[command(after_help = help_text!("frame/get"))]
    Get {
        /// Frame ID returned by thread stack (decimal or 0x); refresh after resume/step.
        #[arg(value_parser = wire::number)]
        frame: u64,
        /// Thread ID belonging to this frame (decimal or 0x).
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
        /// Comma-separated slot:type pairs, e.g. 0:int,2:object (decimal slots).
        #[arg(long, value_delimiter = ',', required = true)]
        registers: Vec<String>,
    },
    /// Write one typed JDWP slot directly; no read-back or undo.
    #[command(after_help = help_text!("frame/set"))]
    Set {
        /// Frame ID returned by thread stack (decimal or 0x); refresh after resume/step.
        #[arg(value_parser = wire::number)]
        frame: u64,
        /// Thread ID belonging to this frame (decimal or 0x).
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
        /// JDWP slot number (decimal, without a prefix).
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

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Ddm {
    /// Query feature names advertised by the target; no capability cache.
    #[command(after_help = help_text!("ddm/features"))]
    Features,
    /// Send one DDM chunk through the existing JDWP connection.
    #[command(after_help = help_text!("ddm/raw"))]
    Raw {
        /// Exactly four ASCII characters, case-sensitive, e.g. VULW.
        #[arg(value_parser = chunk_type)]
        chunk: String,
        /// Hex-encoded chunk payload; omit for an empty payload.
        #[arg(value_name = "HEX", default_value = "")]
        payload_hex: String,
    },
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Ui {
    /// List ViewRoot names in the attached process.
    #[command(after_help = help_text!("ui/windows"))]
    Windows,
    /// Decode a fresh V2 View hierarchy, including exported properties.
    #[command(after_help = help_text!("ui/dump"))]
    Dump {
        /// Exact window name returned by ui windows.
        #[arg(short = 'w', long)]
        window: String,
    },
    /// Redraw a View into a PNG saved by the client; requires the UI thread.
    #[command(after_help = help_text!("ui/capture"))]
    Capture {
        /// Exact window name returned by ui windows.
        #[arg(short = 'w', long)]
        window: String,
        /// Class@hexHash from ui dump, or a fully qualified resource name.
        #[arg(short = 'v', long)]
        view: String,
        /// Client-local PNG path; parent must exist, existing files are refused.
        #[arg(short = 'o', long)]
        #[serde(skip)]
        output: PathBuf,
    },
}

fn chunk_type(value: &str) -> Result<String, String> {
    if value.len() != 4 || !value.is_ascii() {
        return Err("chunk must contain exactly four ASCII characters".into());
    }

    Ok(value.to_owned())
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

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum StringCommand {
    /// Read a string's contents without traversing its implementation fields.
    #[command(after_help = help_text!("string/get"))]
    Get {
        /// Runtime string object ID (decimal or 0x).
        #[arg(value_parser = wire::number)]
        object: u64,
    },
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Thread {
    /// List thread IDs, names, execution status and suspend status.
    #[command(after_help = help_text!("thread/list"))]
    List,
    /// Read a suspended thread's full call stack.
    #[command(after_help = help_text!("thread/stack"))]
    Stack {
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
    },
    /// Register one instruction step and resume the thread; does not wait.
    #[command(after_help = help_text!("thread/step"))]
    #[command(group(clap::ArgGroup::new("depth").required(true).args(["into", "over", "out"])))]
    Step {
        /// Thread ID from thread list or an event (decimal or 0x).
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
    /// Suspend one thread once.
    #[command(after_help = help_text!("thread/suspend"))]
    Suspend {
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
    },
    /// Resume one thread once.
    #[command(after_help = help_text!("thread/resume"))]
    Resume {
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
    },
    /// Query the VM's current suspend count; no locally maintained counter.
    #[command(after_help = help_text!("thread/suspend-count"))]
    SuspendCount {
        #[arg(short = 't', long, value_parser = wire::number)]
        thread: u64,
    },
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Vm {
    /// Send one VM-wide suspend request.
    #[command(after_help = help_text!("vm/suspend"))]
    Suspend,
    /// Send one VM-wide resume request.
    #[command(after_help = help_text!("vm/resume"))]
    Resume,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArrayRange {
    pub start: u32,
    pub length: u32,
}

fn array_range(value: &str) -> Result<ArrayRange, String> {
    let parse_index = |part: &str| {
        part.parse::<u32>()
            .ok()
            .filter(|index| *index <= i32::MAX as u32)
            .filter(|_| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| "range endpoints must be decimal integers in 0..2147483647".to_owned())
    };
    let (start, length) = if let Some((start, end)) = value.split_once(':') {
        let start = parse_index(start)?;
        let end = parse_index(end)?;
        let length = end.checked_sub(start).ok_or("range end must be >= start")?;

        (start, length)
    } else {
        (parse_index(value)?, 1)
    };

    Ok(ArrayRange { start, length })
}

#[derive(Subcommand, Clone, Debug, Deserialize, Serialize)]
pub enum Array {
    /// Query the number of elements.
    #[command(after_help = help_text!("array/length"))]
    Length {
        #[arg(value_parser = wire::number)]
        array: u64,
    },
    /// Read an explicit range; the target reports element types.
    #[command(after_help = help_text!("array/get"))]
    Get {
        #[arg(value_parser = wire::number)]
        array: u64,
        /// One index or a half-open start:end range (decimal).
        #[arg(value_name = "INDEX|START:END", value_parser = array_range)]
        range: ArrayRange,
    },
    /// Write an explicit range directly, without reading or suspending.
    #[command(after_help = help_text!("array/set"))]
    Set {
        #[arg(value_parser = wire::number)]
        array: u64,
        /// One index or a half-open start:end range (decimal).
        #[arg(value_name = "INDEX|START:END", value_parser = array_range)]
        range: ArrayRange,
        /// Exact primitive component type, or object for reference arrays.
        #[arg(long = "type", value_parser = wire::value_tag)]
        tag: u8,
        /// Comma-separated typed values; object IDs/null for reference arrays.
        #[arg(
            long,
            value_delimiter = ',',
            allow_hyphen_values = true,
            value_parser = clap::builder::NonEmptyStringValueParser::new()
        )]
        values: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_examples_match_the_current_parser() {
        let mut directories = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("help")];
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
    fn ddm_and_ui_arguments_are_explicit_and_scoped() {
        for args in [
            vec!["ddm", "features"],
            vec!["ddm", "raw", "VULW"],
            vec!["ui", "windows"],
            vec!["ui", "dump", "-w", "window"],
            vec![
                "ui", "capture", "-w", "window", "-v", "View@1", "-o", "out.png",
            ],
        ] {
            assert!(
                InteractiveArgs::try_parse_from(
                    std::iter::once("yaya").chain(args.iter().copied())
                )
                .is_ok()
            );
            let parsed = Args::try_parse_from(
                std::iter::once("yaya")
                    .chain(args)
                    .chain(["-s", "s_abc123"]),
            )
            .unwrap();
            let EntryCommand::SessionCommand(command) = parsed.command else {
                panic!()
            };
            assert_eq!(command.session(), Some("s_abc123"));
        }
        for args in [
            vec!["ddm", "raw", "ABC"],
            vec!["ddm", "raw", "ABCDE"],
            vec!["ddm", "raw", "你好"],
            vec!["ui", "dump"],
            vec!["ui", "capture", "-w", "window", "-o", "out.png"],
            vec!["ui", "capture", "-w", "window", "-v", "View@1"],
        ] {
            assert!(Args::try_parse_from(std::iter::once("yaya").chain(args)).is_err());
        }
    }

    #[test]
    fn primitive_inspection_requires_explicit_valid_arguments() {
        for args in [
            vec!["suspend"],
            vec!["suspend", "-t", "1", "--all"],
            vec!["thread", "suspend-count"],
            vec!["frame", "this", "1"],
            vec!["array", "get", "1", "--start", "0"],
            vec!["array", "get", "1"],
            vec!["array", "set", "1", "0:0", "--type", "int", "--values="],
            vec!["object", "referrers", "1", "--limit", "2147483648"],
        ] {
            assert!(Args::try_parse_from(std::iter::once("yaya").chain(args)).is_err());
        }
    }

    #[test]
    fn array_ranges_are_half_open_and_bounded() {
        for (text, start, length) in [
            ("2", 2, 1),
            ("2147483647", 2147483647, 1),
            ("2:5", 2, 3),
            ("0:0", 0, 0),
            ("2147483647:2147483647", 2147483647, 0),
        ] {
            let range = array_range(text).unwrap();
            assert_eq!((range.start, range.length), (start, length));
            assert!(InteractiveArgs::try_parse_from(["yaya", "array", "get", "1", text]).is_ok());
        }
        for text in [
            "",
            "2147483648",
            ":3",
            "2:",
            "3:2",
            "-1:2",
            "0x1:2",
            "0:2147483648",
            "1:2:3",
            "+1:2",
        ] {
            assert!(array_range(text).is_err(), "{text}");
        }
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
            vec!["frame", "get", "0x22", "-t", "18", "--registers", "0:int"],
            vec![
                "frame", "set", "34", "--thread", "0x12", "0", "--type", "int", "--value", "42",
            ],
        ] {
            assert!(InteractiveArgs::try_parse_from(std::iter::once("yaya").chain(args)).is_ok());
        }
        for args in [
            vec!["frame", "get", "0x22", "--registers", "0:int"],
            vec![
                "frame",
                "get",
                "s_abc123:0x12:0x22",
                "-t",
                "0x12",
                "--registers",
                "0:int",
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
            vec!["thread", "list", "-s", "s_abc123"],
            vec!["class", "find", "Card", "--session", "s_abc123"],
        ] {
            assert!(InteractiveArgs::try_parse_from(std::iter::once("yaya").chain(args)).is_err());
        }
        assert!(InteractiveArgs::try_parse_from(["yaya", "thread", "list"]).is_ok());
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
    fn daemon_control_uses_existing_session_arguments() {
        for args in [
            vec!["daemon", "stop", "-s", "s_abc123"],
            vec!["daemon", "--session", "s_abc123", "stop"],
        ] {
            let parsed = Args::try_parse_from(std::iter::once("yaya").chain(args)).unwrap();
            let EntryCommand::Daemon {
                session,
                command: DaemonCommand::Stop,
            } = parsed.command
            else {
                panic!("expected daemon stop");
            };
            assert_eq!(session.session.as_deref(), Some("s_abc123"));
        }
        assert!(Args::try_parse_from(["yaya", "daemon"]).is_err());
        assert!(InteractiveArgs::try_parse_from(["yaya", "daemon", "stop"]).is_err());
    }

    #[test]
    fn session_is_scoped_to_existing_session_commands() {
        for args in [
            vec!["thread", "list", "-s", "s_abc123"],
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
                "0:int",
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
            vec!["-s", "s_abc123", "thread", "list"],
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
    fn execution_commands_are_scoped_by_target() {
        for action in ["suspend", "resume"] {
            assert!(Args::try_parse_from(["yaya", "thread", action, "-t", "1"]).is_ok());
            assert!(Args::try_parse_from(["yaya", "thread", action]).is_err());
            assert!(Args::try_parse_from(["yaya", "vm", action]).is_ok());
            assert!(Args::try_parse_from(["yaya", "vm", action, "-t", "1"]).is_err());
            assert!(Args::try_parse_from(["yaya", action, "-t", "1"]).is_err());
        }
        for old in ["threads", "stack", "step"] {
            assert!(Args::try_parse_from(["yaya", old]).is_err());
        }
    }

    #[test]
    fn validates_exclusive_modes() {
        assert!(
            Args::try_parse_from(["yaya", "events", "--wait", "2s", "--collect", "3s"]).is_err()
        );
        assert!(Args::try_parse_from(["yaya", "thread", "step", "--thread", "1"]).is_err());
        for pair in [
            ["--into", "--over"],
            ["--into", "--out"],
            ["--over", "--out"],
        ] {
            assert!(
                Args::try_parse_from(["yaya", "thread", "step", "--thread", "1", pair[0], pair[1]])
                    .is_err()
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
