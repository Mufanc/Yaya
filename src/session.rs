pub mod client;
pub mod daemon;
mod file;
pub mod interactive;
mod protocol;
pub mod server;
pub mod transport;

use std::fs::{self, DirBuilder};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::PathBuf;

use anyhow::{Context, bail, ensure};

use crate::commands::Data;

pub fn validate_id(id: &str) -> anyhow::Result<()> {
    ensure!(
        id.len() == 8
            && id.starts_with("s_")
            && id[2..]
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "invalid session ID; expected s_ plus six lowercase letters/digits"
    );

    Ok(())
}

pub fn selected(explicit: Option<String>) -> anyhow::Result<String> {
    let id = explicit
        .or_else(|| std::env::var("YAYA_SESSION").ok().filter(|s| !s.is_empty()))
        .context("specify --session or YAYA_SESSION")?;

    validate_id(&id)?;
    Ok(id)
}

fn root() -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join("yaya");

    match DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }

    let metadata = fs::symlink_metadata(&path)?;
    // SAFETY: geteuid has no arguments or failure mode.
    let uid = unsafe { libc::geteuid() };

    ensure!(
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "yaya temp root must be an owned private directory: {}",
        path.display()
    );

    Ok(path)
}

fn directory(id: &str) -> anyhow::Result<PathBuf> {
    validate_id(id)?;
    Ok(root()?.join(id))
}

pub(crate) fn socket_path(id: &str) -> anyhow::Result<PathBuf> {
    let path = directory(id)?.join("control.sock");

    ensure!(
        path.as_os_str().as_encoded_bytes().len() < 104,
        "Unix socket path too long; use a shorter TMPDIR for all yaya invocations"
    );

    Ok(path)
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("control.sock"));
        let _ = fs::remove_dir(&self.0);
    }
}

pub fn new_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

    let mut id = String::with_capacity(8);

    id.push_str("s_");

    for _ in 0..6 {
        id.push(ALPHABET[fastrand::usize(..ALPHABET.len())] as char);
    }

    id
}

fn allocate() -> anyhow::Result<(String, Cleanup)> {
    let root = root()?;

    for _ in 0..100 {
        let id = new_id();
        let path = root.join(&id);

        match DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok((id, Cleanup(path))),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    bail!("could not allocate a unique session directory")
}

pub fn sessions() -> anyhow::Result<Data> {
    let mut sessions = Vec::new();

    for entry in fs::read_dir(root()?)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if validate_id(&name).is_ok() {
            sessions.push(name);
        }
    }

    sessions.sort();

    Ok(Data::Sessions { sessions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_session_ids_match_the_public_format() {
        for _ in 0..32 {
            validate_id(&new_id()).unwrap();
        }
    }

    #[test]
    fn session_validation_prevents_path_traversal() {
        assert!(validate_id("s_k7m2px").is_ok());
        for id in ["../a", "s_ABC123", "s_a/bcde", "s_1234567", "s_💣a"] {
            assert!(validate_id(id).is_err());
        }
        assert_eq!(selected(Some("s_abc123".into())).unwrap(), "s_abc123");
    }
}
