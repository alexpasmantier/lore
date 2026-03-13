use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub ingestion: IngestionConfig,
    #[serde(default)]
    pub consolidation: ConsolidationConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub claude: ClaudeConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestionConfig {
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_claude_model")]
    pub claude_model: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ConsolidationConfig {
    #[serde(default = "default_consolidation_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,
    /// Similarity above which topics are merged (must be >= similarity_threshold).
    #[serde(default = "default_merge_threshold")]
    pub merge_threshold: f32,
    #[serde(default = "default_prune_age_days")]
    pub prune_age_days: u32,
    /// Minimum relevance score below which fragments may be pruned.
    #[serde(default = "default_min_relevance_prune")]
    pub min_relevance_prune: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeConfig {
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

fn default_poll_interval() -> u64 {
    30
}
fn default_batch_size() -> usize {
    100
}
fn default_claude_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}
fn default_consolidation_interval() -> u64 {
    7200
}
fn default_similarity_threshold() -> f32 {
    0.8
}
fn default_merge_threshold() -> f32 {
    0.85
}
fn default_prune_age_days() -> u32 {
    30
}
fn default_min_relevance_prune() -> f32 {
    0.02
}
fn default_db_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/.lore/memory.db", home)
}
fn default_api_key_env() -> String {
    "ANTHROPIC_API_KEY".to_string()
}

impl Default for IngestionConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: default_poll_interval(),
            batch_size: default_batch_size(),
            claude_model: default_claude_model(),
        }
    }
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_consolidation_interval(),
            similarity_threshold: default_similarity_threshold(),
            merge_threshold: default_merge_threshold(),
            prune_age_days: default_prune_age_days(),
            min_relevance_prune: default_min_relevance_prune(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
        }
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_api_key_env(),
        }
    }
}

// Note: Can't use #[derive(Default)] because sub-structs have custom defaults
// that differ from the type's inherent Default (e.g. non-empty strings).
#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self {
            ingestion: IngestionConfig::default(),
            consolidation: ConsolidationConfig::default(),
            database: DatabaseConfig::default(),
            claude: ClaudeConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn db_path(&self) -> PathBuf {
        let path = self
            .database
            .path
            .replace('~', &std::env::var("HOME").unwrap_or_default());
        PathBuf::from(path)
    }

    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.claude.api_key_env).ok()
    }
}
