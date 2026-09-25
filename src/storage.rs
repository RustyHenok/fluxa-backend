//! Artifact storage abstraction for generated export files. The default
//! backend stores artifacts on the local filesystem under a configurable
//! directory; the trait is designed so an S3-compatible backend can be added
//! without touching callers.

use std::path::{Path, PathBuf};

use crate::config::Cli;
use crate::error::{AppError, AppResult};

pub trait ArtifactStore {
    fn put(&self, key: &str, bytes: &[u8]) -> impl Future<Output = AppResult<()>> + Send;
    fn get(&self, key: &str) -> impl Future<Output = AppResult<Option<Vec<u8>>>> + Send;
    fn delete(&self, key: &str) -> impl Future<Output = AppResult<()>> + Send;
}

#[derive(Debug, Clone)]
pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, key: &str) -> AppResult<PathBuf> {
        validate_key(key)?;
        Ok(self.root.join(key))
    }
}

impl ArtifactStore for LocalFsStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> AppResult<()> {
        let path = self.resolve(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                AppError::internal(format!("failed to create artifact directory: {error}"))
            })?;
        }
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|error| AppError::internal(format!("failed to write artifact: {error}")))
    }

    async fn get(&self, key: &str) -> AppResult<Option<Vec<u8>>> {
        let path = self.resolve(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AppError::internal(format!(
                "failed to read artifact: {error}"
            ))),
        }
    }

    async fn delete(&self, key: &str) -> AppResult<()> {
        let path = self.resolve(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::internal(format!(
                "failed to delete artifact: {error}"
            ))),
        }
    }
}

#[derive(Clone)]
pub enum AnyArtifactStore {
    LocalFs(LocalFsStore),
}

impl AnyArtifactStore {
    pub fn from_config(config: &Cli) -> Self {
        Self::LocalFs(LocalFsStore::new(Path::new(&config.artifact_storage_dir)))
    }
}

impl ArtifactStore for AnyArtifactStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> AppResult<()> {
        match self {
            Self::LocalFs(store) => store.put(key, bytes).await,
        }
    }

    async fn get(&self, key: &str) -> AppResult<Option<Vec<u8>>> {
        match self {
            Self::LocalFs(store) => store.get(key).await,
        }
    }

    async fn delete(&self, key: &str) -> AppResult<()> {
        match self {
            Self::LocalFs(store) => store.delete(key).await,
        }
    }
}

/// Keys are generated internally, but validate defensively so a corrupted
/// result payload can never escape the storage root.
fn validate_key(key: &str) -> AppResult<()> {
    let valid = !key.is_empty()
        && !key.starts_with('/')
        && !key.split('/').any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || !segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        });

    if valid {
        Ok(())
    } else {
        Err(AppError::internal(format!("invalid artifact key: {key}")))
    }
}

#[cfg(test)]
mod tests {
    use super::validate_key;

    #[test]
    fn key_validation_rejects_traversal() {
        assert!(validate_key("tenant/job.json").is_ok());
        assert!(validate_key("a-b_c.1/d.csv").is_ok());
        assert!(validate_key("../etc/passwd").is_err());
        assert!(validate_key("a/../b").is_err());
        assert!(validate_key("/absolute").is_err());
        assert!(validate_key("a//b").is_err());
        assert!(validate_key("").is_err());
        assert!(validate_key("a/b c").is_err());
    }
}
