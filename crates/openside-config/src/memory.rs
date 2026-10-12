use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File names a local static-embedding model directory must contain; kept identical
/// to `openside_memory::{TOKENIZER_FILE, MODEL_FILE}`.
pub const EMBEDDING_MODEL_FILES: [&str; 2] = ["tokenizer.json", "model.safetensors"];

/// Retention applied to memories written without an explicit choice.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum MemoryRetentionSetting {
    /// Never purged automatically.
    #[default]
    #[serde(rename = "permanent")]
    Permanent,
    /// Purged 90 days after the last write.
    #[serde(rename = "days_90")]
    Days90,
    /// Purged 30 days after the last write.
    #[serde(rename = "days_30")]
    Days30,
}

/// Local memory settings. Browsing history always uses 30-day retention.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    /// Retention for notes written without one: `permanent`, `days_90` or `days_30`.
    pub default_retention: MemoryRetentionSetting,
    /// Index visited pages (public URL, title, short text) as history memories.
    pub history: bool,
    /// Local embedding model; absent weights mean lexical-only search.
    pub embedding: EmbeddingConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            default_retention: MemoryRetentionSetting::default(),
            history: true,
            embedding: EmbeddingConfig::default(),
        }
    }
}

/// Weights are only obtained by `openside memory model fetch`; nothing downloads implicitly.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EmbeddingConfig {
    /// Directory with `tokenizer.json` and `model.safetensors`
    /// (default `<data_dir>/memory/model`).
    pub model_dir: Option<PathBuf>,
}

impl MemoryConfig {
    /// Configured model directory, else `<data_dir>/memory/model`.
    pub fn model_dir(&self, data_dir: &Path) -> PathBuf {
        self.embedding.model_dir.clone().unwrap_or_else(|| {
            crate::data_layout::DataPath::MemoryModel
                .path(data_dir)
                .join("model")
        })
    }
}

/// Doctor check `memory.embeddings`: lexical-only when model files are missing. Present files
/// are not validated here; the daemon reports the effective mode in `GET /v1/memory/health`.
pub(crate) fn doctor_check(report: &mut crate::doctor::DoctorReport, loaded: &crate::Loaded) {
    use crate::doctor::{Check, Status};
    let dir = loaded.config.memory.model_dir(&loaded.paths.data_dir);
    let missing: Vec<&str> = EMBEDDING_MODEL_FILES
        .into_iter()
        .filter(|name| !dir.join(name).is_file())
        .collect();
    report.checks.push(if missing.is_empty() {
        Check {
            id: "memory.embeddings".into(),
            status: Status::Ok,
            summary: format!(
                "model files present in {} (validity not checked; GET /v1/memory/health shows the effective mode)",
                dir.display()
            ),
            next_step: None,
        }
    } else {
        Check {
            id: "memory.embeddings".into(),
            status: Status::Warn,
            summary: format!(
                "lexical-only: {} missing in {}; memory search uses full-text matching only",
                missing.join(", "),
                dir.display()
            ),
            next_step: Some(
                "Run `openside memory model fetch <hugging-face-repo>` with a WordPiece static (model2vec-style) embedding model, or set memory.embedding.model_dir to a directory containing tokenizer.json and model.safetensors; then restart opensided."
                    .into(),
            ),
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{doctor::Status, parse_config, Config, Loaded, Paths};

    fn loaded(root: &Path, config: Config) -> Loaded {
        Loaded {
            config,
            paths: Paths {
                config_file: root.join("config.toml"),
                data_dir: root.join("data"),
                state_dir: root.join("state"),
                runtime_dir: root.join("run"),
            },
            source: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn memory_section_parses_and_rejects_unknown_keys() {
        let path = Path::new("config.toml");
        let config = parse_config(
            path,
            "[memory]\ndefault_retention = \"days_30\"\nhistory = false\n[memory.embedding]\nmodel_dir = \"/models/static\"\n",
        )
        .unwrap();
        assert_eq!(
            config.memory.default_retention,
            MemoryRetentionSetting::Days30
        );
        assert!(!config.memory.history);
        assert_eq!(
            config.memory.model_dir(Path::new("/data")),
            PathBuf::from("/models/static")
        );
        let defaults = Config::default().memory;
        assert!(defaults.history);
        assert_eq!(
            defaults.default_retention,
            MemoryRetentionSetting::Permanent
        );
        assert_eq!(
            defaults.model_dir(Path::new("/data")),
            PathBuf::from("/data/memory/model")
        );
        let error = parse_config(path, "[memory.embedding]\nurl = \"https://example.test\"\n")
            .err()
            .unwrap();
        assert!(error.key.starts_with("memory.embedding"), "{}", error.key);
        assert!(parse_config(path, "[memory]\ndefault_retention = \"forever\"\n").is_err());
    }

    #[test]
    fn doctor_reports_lexical_only_until_model_files_exist() {
        let root = tempfile::tempdir().unwrap();
        let loaded = loaded(root.path(), Config::default());
        let mut report = crate::doctor::DoctorReport::default();
        doctor_check(&mut report, &loaded);
        let check = &report.checks[0];
        assert_eq!(
            (check.id.as_str(), check.status),
            ("memory.embeddings", Status::Warn)
        );
        assert!(
            check.summary.starts_with("lexical-only"),
            "{}",
            check.summary
        );
        assert!(check
            .next_step
            .as_deref()
            .unwrap()
            .contains("openside memory model fetch"));

        let dir = root.path().join("data/memory/model");
        std::fs::create_dir_all(&dir).unwrap();
        for name in EMBEDDING_MODEL_FILES {
            std::fs::write(dir.join(name), b"{}").unwrap();
        }
        let mut report = crate::doctor::DoctorReport::default();
        doctor_check(&mut report, &loaded);
        assert_eq!(report.checks[0].status, Status::Ok);
        assert!(report.checks[0].summary.starts_with("model files present"));
        assert!(report
            .render_text()
            .contains("[ok] memory.embeddings: model files present"));
    }
}
