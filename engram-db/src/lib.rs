pub mod edge;
pub mod embedding;
pub mod fragment;
pub mod query;
pub mod relevance;
pub mod storage;

// Re-export primary types for convenience
pub use edge::{Edge, EdgeId, EdgeKind};
pub use embedding::{cosine_similarity, Embedder};
pub use fragment::{Fragment, FragmentId, ScoredFragment, Tree};
pub use query::EngramDb;
pub use relevance::{compute_relevance, SEMANTIC_WEIGHT};
pub use storage::Storage;
