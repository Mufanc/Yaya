use std::path::Path;

use anyhow::Context;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use crate::commands::Data;

pub(super) async fn save(path: &Path, bytes: &[u8]) -> anyhow::Result<Data> {
    let path = std::path::absolute(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .with_context(|| format!("cannot create output file {}", path.display()))?;
    let result = async {
        file.write_all(bytes).await?;
        file.flush().await
    }
    .await;

    drop(file);

    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error).context("cannot write output file");
    }

    Ok(Data::FileSaved {
        path,
        bytes: bytes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saves_arbitrary_bytes_and_refuses_overwrite() {
        let path = std::env::temp_dir().join(format!(
            "yaya-capture-{}-{}.png",
            std::process::id(),
            fastrand::u64(..)
        ));
        let bytes = b"arbitrary file contents";
        assert!(!path.exists());
        assert!(matches!(
            save(&path, bytes).await.unwrap(),
            Data::FileSaved { .. }
        ));
        assert!(save(&path, bytes).await.is_err());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
        tokio::fs::remove_file(path).await.unwrap();
    }
}
