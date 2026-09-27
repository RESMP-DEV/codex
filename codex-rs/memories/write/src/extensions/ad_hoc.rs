use crate::memory_extensions_root;
use codex_utils_path::write_atomically;
use std::path::Path;
use tokio::io::AsyncWriteExt;

pub(super) const INSTRUCTIONS: &str =
    include_str!("../../templates/extensions/ad_hoc/instructions.md");
/// Byte-for-byte snapshot of the retired managed template, including its trailing newline.
/// Changing it would prevent the one-time migration from recognizing installed legacy content.
const LEGACY_INSTRUCTIONS: &str =
    include_str!("../../templates/extensions/ad_hoc/instructions_legacy.md");

pub(super) async fn seed_instructions(memory_root: &Path) -> std::io::Result<()> {
    let extension_root = memory_extensions_root(memory_root).join("ad_hoc");
    let instructions_path = extension_root.join("instructions.md");

    tokio::fs::create_dir_all(&extension_root).await?;
    match tokio::fs::read_to_string(&instructions_path).await {
        Ok(existing) if existing == INSTRUCTIONS => Ok(()),
        Ok(existing) if existing == LEGACY_INSTRUCTIONS => {
            tokio::task::spawn_blocking(move || write_atomically(&instructions_path, INSTRUCTIONS))
                .await
                .map_err(std::io::Error::other)?
        }
        Ok(_) => {
            tracing::debug!(
                path = %instructions_path.display(),
                "preserving custom ad-hoc memory instructions"
            );
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut file = match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&instructions_path)
                .await
            {
                Ok(file) => file,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
                Err(err) => return Err(err),
            };
            file.write_all(INSTRUCTIONS.as_bytes()).await?;
            file.flush().await
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
#[path = "ad_hoc_tests.rs"]
mod tests;
