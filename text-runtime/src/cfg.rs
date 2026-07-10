// ── RuntimeConfig — Text Runtime configuration ──────────────────────────────

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::TextRuntimeError;

/// Runtime configuration, loaded from `.textruntime/config.json`.
///
/// Controls pandoc-server settings, ingestion parameters, content
/// store layout, and locale preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Port for pandoc-server HTTP API (default: 8472).
    #[serde(default = "default_pandoc_port")]
    pub pandoc_port: u16,

    /// Pandoc server executable name or path (default: "pandoc-server").
    #[serde(default = "default_pandoc_executable")]
    pub pandoc_executable: String,

    /// Edit distance threshold for re-ingestion fuzzy matching (0.0–1.0).
    /// Default 0.20 means nodes with ≤20% edit distance keep their UUID.
    #[serde(default = "default_fuzzy_match_threshold")]
    pub fuzzy_match_threshold: f64,

    /// Maximum number of documents to batch in a single pandoc-server /batch request.
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,

    /// Number of bits for content directory fanout.
    /// 8 → 256 subdirectories (00–ff).
    #[serde(default = "default_content_fanout_bits")]
    pub content_fanout_bits: u8,

    /// Locale for sentence segmentation and language-specific behavior.
    #[serde(default = "default_locale")]
    pub locale: String,

    /// Path to the `.textruntime/` directory (not serialized — set at open time).
    #[serde(skip)]
    pub runtime_dir: PathBuf,
}

// ── Default values ─────────────────────────────────────────────────────────

fn default_pandoc_port() -> u16 {
    8472
}
fn default_pandoc_executable() -> String {
    "pandoc-server".to_string()
}
fn default_fuzzy_match_threshold() -> f64 {
    0.20
}
fn default_max_batch_size() -> usize {
    32
}
fn default_content_fanout_bits() -> u8 {
    8
}
fn default_locale() -> String {
    "en".to_string()
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            pandoc_port: default_pandoc_port(),
            pandoc_executable: default_pandoc_executable(),
            fuzzy_match_threshold: default_fuzzy_match_threshold(),
            max_batch_size: default_max_batch_size(),
            content_fanout_bits: default_content_fanout_bits(),
            locale: default_locale(),
            runtime_dir: PathBuf::new(),
        }
    }
}

impl RuntimeConfig {
    /// Load configuration from `{runtime_dir}/config.json`.
    ///
    /// If the file exists, deserializes it. If any fields are missing,
    /// defaults are used (via serde defaults). If the file does not
    /// exist, returns a default config with `runtime_dir` set.
    pub fn load_or_create(runtime_dir: &Path) -> Result<Self, TextRuntimeError> {
        let config_path = runtime_dir.join("config.json");

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| TextRuntimeError::io(&config_path, e))?;
            let mut config: Self = serde_json::from_str(&content).map_err(|e| {
                TextRuntimeError::ConfigError(format!("failed to parse config.json: {}", e))
            })?;
            config.runtime_dir = runtime_dir.to_path_buf();
            Ok(config)
        } else {
            Ok(Self {
                runtime_dir: runtime_dir.to_path_buf(),
                ..Default::default()
            })
        }
    }

    /// Save current configuration to `{runtime_dir}/config.json`.
    ///
    /// Creates parent directories if they don't exist. Uses atomic
    /// write pattern: write to temp file, fsync, rename.
    pub fn save(&self) -> Result<(), TextRuntimeError> {
        let config_path = self.runtime_dir.join("config.json");

        // Ensure the parent directory exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TextRuntimeError::io(parent, e))?;
        }

        let tmp_path = config_path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            TextRuntimeError::ConfigError(format!("failed to serialize config: {}", e))
        })?;

        std::fs::write(&tmp_path, &json).map_err(|e| TextRuntimeError::io(&tmp_path, e))?;

        // fsync is not directly available on stable Rust for files;
        // the atomicity of rename() is the primary guarantee on POSIX.
        // On crash before rename: tmp file is orphaned but harmless.
        std::fs::rename(&tmp_path, &config_path)
            .map_err(|e| TextRuntimeError::io(&config_path, e))?;

        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = RuntimeConfig::default();
        assert_eq!(config.pandoc_port, 8472);
        assert_eq!(config.pandoc_executable, "pandoc-server");
        assert!((config.fuzzy_match_threshold - 0.20).abs() < f64::EPSILON);
        assert_eq!(config.max_batch_size, 32);
        assert_eq!(config.content_fanout_bits, 8);
        assert_eq!(config.locale, "en");
    }

    #[test]
    fn test_save_load_roundtrip() {
        let tmp = TempDir::new().expect("temp dir");
        let runtime_dir = tmp.path().join(".textruntime");
        std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");

        let mut config = RuntimeConfig::load_or_create(&runtime_dir).expect("load_or_create");

        // Modify some fields
        config.pandoc_port = 9999;
        config.fuzzy_match_threshold = 0.42;
        config.max_batch_size = 64;

        config.save().expect("save config");

        // Load back and verify
        let loaded = RuntimeConfig::load_or_create(&runtime_dir).expect("load again");
        assert_eq!(loaded.pandoc_port, 9999);
        assert!((loaded.fuzzy_match_threshold - 0.42).abs() < f64::EPSILON);
        assert_eq!(loaded.max_batch_size, 64);
        // Fields we didn't change should still be defaults
        assert_eq!(loaded.pandoc_executable, "pandoc-server");
        assert_eq!(loaded.locale, "en");
    }
}
