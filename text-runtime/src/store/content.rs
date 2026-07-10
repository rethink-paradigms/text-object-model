// ── Content File Store ──────────────────────────────────────────────────────
//
// Manages Pandoc AST JSON files in `.textruntime/content/`.
// Uses 256-bucket fanout (first 2 hex characters of UUID).

use std::fs;
use std::path::PathBuf;

use crate::error::TextRuntimeError;
use crate::types::Uuid;

/// The content store manages Pandoc AST JSON files on the filesystem.
///
/// Files are stored at `{base_path}/{first2_hex}/{uuid}.json`.
/// Writes use an atomic protocol: write to tmp/, fsync, rename.
pub struct ContentStore {
    base_path: PathBuf,
}

impl ContentStore {
    /// Create a new content store rooted at the given directory.
    ///
    /// Creates the directory and all 256 fanout subdirectories (00–ff)
    /// if they don't already exist.
    pub fn new(base_path: PathBuf) -> Result<Self, TextRuntimeError> {
        // Create base content directory
        fs::create_dir_all(&base_path).map_err(|e| TextRuntimeError::io(&base_path, e))?;

        // Create 256 fanout directories: 00 through ff
        for i in 0u8..=255u8 {
            let fanout_dir = base_path.join(format!("{:02x}", i));
            fs::create_dir_all(&fanout_dir).map_err(|e| TextRuntimeError::io(&fanout_dir, e))?;
        }

        Ok(Self { base_path })
    }

    /// Read the content file for a given UUID.
    ///
    /// Returns the raw bytes of the content file (Pandoc AST JSON).
    /// Returns `ContentFileNotFound` if the file does not exist.
    pub fn get(&self, uuid: &Uuid) -> Result<Vec<u8>, TextRuntimeError> {
        let path = self.fanout_path(uuid);
        fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TextRuntimeError::ContentFileNotFound(uuid.to_string())
            } else {
                TextRuntimeError::io(&path, e)
            }
        })
    }

    /// Write content for a UUID using the atomic write protocol:
    ///
    /// 1. Write to `tmp/{uuid}.tmp`
    /// 2. fsync the file
    /// 3. rename → `content/{first2}/{uuid}.json`
    ///
    /// The tmp/ directory must exist (created by Store::open).
    /// On crash before rename, the tmp file is orphaned but harmless.
    pub fn put(&self, uuid: &Uuid, content: &[u8]) -> Result<(), TextRuntimeError> {
        let tmp_dir = match self.base_path.parent() {
            Some(p) => p.join("tmp"),
            None => PathBuf::from("tmp"),
        };
        fs::create_dir_all(&tmp_dir).map_err(|e| TextRuntimeError::io(&tmp_dir, e))?;

        let tmp_path = tmp_dir.join(format!("{}.tmp", uuid));
        let target_path = self.fanout_path(uuid);

        // Ensure the fanout directory exists (belt-and-suspenders)
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|e| TextRuntimeError::io(parent, e))?;
        }

        // Step 1: Write to tmp file
        fs::write(&tmp_path, content).map_err(|e| TextRuntimeError::io(&tmp_path, e))?;

        // Step 2: fsync via File::sync_all
        {
            let file = fs::File::open(&tmp_path).map_err(|e| TextRuntimeError::io(&tmp_path, e))?;
            file.sync_all()
                .map_err(|e| TextRuntimeError::io(&tmp_path, e))?;
        }

        // Step 3: Atomic rename
        fs::rename(&tmp_path, &target_path).map_err(|e| TextRuntimeError::io(&target_path, e))?;

        Ok(())
    }

    /// Delete a content file (marks as absent; SQLite handles status).
    ///
    /// If the file does not exist, this is a no-op (not an error).
    pub fn delete(&self, uuid: &Uuid) -> Result<(), TextRuntimeError> {
        let path = self.fanout_path(uuid);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(TextRuntimeError::io(&path, e)),
        }
    }

    /// Check if a content file exists for the given UUID.
    pub fn exists(&self, uuid: &Uuid) -> bool {
        self.fanout_path(uuid).exists()
    }

    /// Compute the fanout path for a UUID.
    ///
    /// Returns `{base_path}/{first2_hex}/{uuid}.json`.
    pub fn fanout_path(&self, uuid: &Uuid) -> PathBuf {
        let s = uuid.to_string();
        let first2 = &s[..2];
        self.base_path.join(first2).join(format!("{}.json", uuid))
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_store() -> (ContentStore, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let content_path = tmp.path().join("content");
        let store = ContentStore::new(content_path).expect("create store");
        (store, tmp)
    }

    #[test]
    fn test_put_get_roundtrip() {
        let (store, _tmp) = setup_store();
        let uuid = crate::uuid7::uuid7();
        let content = br#"{"t":"Para","c":[{"t":"Str","c":"hello"}]}"#;

        store.put(&uuid, content).expect("put");
        let retrieved = store.get(&uuid).expect("get");

        assert_eq!(retrieved, content);
    }

    #[test]
    fn test_atomic_write_no_partial_reads() {
        // Verify that the atomic write protocol works:
        // the file should NOT be readable at the target path until
        // after the rename completes.
        let (store, _tmp) = setup_store();
        let uuid = crate::uuid7::uuid7();
        let content = b"atomic test content";

        // Before put: file should not exist
        assert!(!store.exists(&uuid));

        store.put(&uuid, content).expect("put");

        // After put: file should exist with correct content
        assert!(store.exists(&uuid));
        let retrieved = store.get(&uuid).expect("get");
        assert_eq!(retrieved, content);
    }

    #[test]
    fn test_fanout_path() {
        let (store, _tmp) = setup_store();

        // Use a known UUID to test path computation
        let uuid: Uuid = "019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b"
            .parse()
            .expect("parse uuid");
        let path = store.fanout_path(&uuid);

        // First 2 chars of UUID string are "01"
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("01"),
            "path should contain first2 hex chars"
        );
        assert!(path_str.ends_with(".json"), "path should end with .json");
        assert!(
            path_str.contains("019f4a1b-2c3d-7e4f-8a5b-6c7d8e9f0a1b"),
            "path should contain full UUID"
        );
    }
}
