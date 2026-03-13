pub mod edge;
pub mod embedding;
pub mod fragment;
pub mod query;
pub mod relevance;
pub mod storage;

use std::path::PathBuf;

/// Cross-platform path to `~/.lore/`.
pub fn lore_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lore")
}

// Re-export primary types for convenience
pub use edge::{Edge, EdgeId, EdgeKind};
pub use embedding::{cosine_similarity, Embedder};
pub use fragment::{Fragment, FragmentId, ScoredFragment, Tree};
pub use query::LoreDb;
pub use relevance::{compute_relevance, SEMANTIC_WEIGHT};
pub use storage::{StagedSession, StagedTurn, Storage};
