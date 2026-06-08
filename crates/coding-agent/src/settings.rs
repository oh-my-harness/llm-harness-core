use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Compaction behaviour settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionSettings {
    /// Enable auto-compaction (default: true).
    pub enabled: Option<bool>,
    /// Tokens to reserve for the LLM response (default: 16384).
    pub reserve_tokens: Option<u32>,
    /// Tokens of recent context to always keep (default: 20000).
    pub keep_recent_tokens: Option<u32>,
}

/// Auto-retry settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetrySettings {
    /// Enable auto-retry on transient errors (default: true).
    pub enabled: Option<bool>,
    /// Maximum retry attempts (default: 3).
    pub max_retries: Option<u32>,
    /// Base delay in ms for exponential backoff (default: 2000).
    pub base_delay_ms: Option<u64>,
}

/// Top-level settings structure (project + global merged).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Default LLM provider (e.g. "anthropic").
    pub default_provider: Option<String>,
    /// Default model ID.
    pub default_model: Option<String>,
    /// Default thinking level ("off" | "minimal" | "low" | "medium" | "high" | "xhigh").
    pub default_thinking_level: Option<String>,
    /// Compaction behaviour.
    pub compaction: Option<CompactionSettings>,
    /// Auto-retry behaviour.
    pub retry: Option<RetrySettings>,
    /// Maximum output tokens per LLM call.
    pub max_tokens: Option<u32>,
    /// Custom session storage directory.
    pub session_dir: Option<String>,
    /// Active tool names (e.g. ["read","bash","edit","write","grep"]).
    /// When absent the default set is used.
    pub active_tools: Option<Vec<String>>,
}

impl Settings {
    /// Merge `other` on top of `self` (other takes precedence for non-None fields).
    pub fn merge(mut self, other: Settings) -> Settings {
        macro_rules! take {
            ($field:ident) => {
                if other.$field.is_some() {
                    self.$field = other.$field;
                }
            };
        }
        take!(default_provider);
        take!(default_model);
        take!(default_thinking_level);
        take!(compaction);
        take!(retry);
        take!(max_tokens);
        take!(session_dir);
        take!(active_tools);
        self
    }
}

// ── SettingsManager ────────────────────────────────────────────────────────────

/// Loads and merges global + project settings.
pub struct SettingsManager {
    merged: Settings,
}

impl SettingsManager {
    /// Load settings from global config dir and optional project dir.
    ///
    /// Project settings take precedence over global settings.
    pub fn load(global_config_dir: &Path, project_dir: Option<&Path>) -> Self {
        let global = read_settings_file(&global_config_dir.join("settings.json"));
        let project = project_dir
            .map(|d| read_settings_file(&d.join(".coding-agent").join("settings.json")))
            .unwrap_or_default();
        let merged = global.merge(project);
        Self { merged }
    }

    /// Return the merged settings.
    pub fn settings(&self) -> &Settings {
        &self.merged
    }

    /// Resolve the model to use, falling back to a default.
    pub fn resolved_model(&self, default: &str) -> String {
        self.merged
            .default_model
            .clone()
            .unwrap_or_else(|| default.to_string())
    }
}

fn read_settings_file(path: &Path) -> Settings {
    if !path.exists() {
        return Settings::default();
    }
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

/// Return the default global config directory for this application.
pub fn default_config_dir() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("coding-agent")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn settings_merge_project_wins() {
        let base = Settings {
            default_model: Some("gpt-4".into()),
            max_tokens: Some(1024),
            ..Default::default()
        };
        let project = Settings {
            default_model: Some("claude-3".into()),
            ..Default::default()
        };
        let merged = base.merge(project);
        assert_eq!(merged.default_model.as_deref(), Some("claude-3"));
        assert_eq!(merged.max_tokens, Some(1024));
    }

    #[test]
    fn settings_manager_loads_missing_file_as_default() {
        let dir = TempDir::new().unwrap();
        let mgr = SettingsManager::load(dir.path(), None);
        assert!(mgr.settings().default_model.is_none());
    }

    #[test]
    fn settings_manager_reads_global_json() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"default_model":"claude-sonnet-4-6","max_tokens":4096}"#,
        )
        .unwrap();
        let mgr = SettingsManager::load(dir.path(), None);
        assert_eq!(
            mgr.settings().default_model.as_deref(),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(mgr.settings().max_tokens, Some(4096));
    }

    #[test]
    fn settings_manager_project_overrides_global() {
        let global_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        std::fs::write(
            global_dir.path().join("settings.json"),
            r#"{"default_model":"global-model"}"#,
        )
        .unwrap();

        let proj_settings_dir = project_dir.path().join(".coding-agent");
        std::fs::create_dir(&proj_settings_dir).unwrap();
        std::fs::write(
            proj_settings_dir.join("settings.json"),
            r#"{"default_model":"project-model"}"#,
        )
        .unwrap();

        let mgr = SettingsManager::load(global_dir.path(), Some(project_dir.path()));
        assert_eq!(
            mgr.settings().default_model.as_deref(),
            Some("project-model")
        );
    }

    #[test]
    fn resolved_model_returns_default_when_unset() {
        let dir = TempDir::new().unwrap();
        let mgr = SettingsManager::load(dir.path(), None);
        assert_eq!(mgr.resolved_model("fallback-model"), "fallback-model");
    }
}
