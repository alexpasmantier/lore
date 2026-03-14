use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fragment::FragmentId;

/// Unique identifier for an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub Uuid);

impl EdgeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for EdgeId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EdgeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The kind of relationship between two fragments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// Parent→child (tree structure)
    Hierarchical,
    /// Cross-branch semantic link
    Associative,
    /// Time-ordered within a topic
    Temporal,
    /// Newer fragment replaces older
    Supersedes,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Hierarchical => "hierarchical",
            EdgeKind::Associative => "associative",
            EdgeKind::Temporal => "temporal",
            EdgeKind::Supersedes => "supersedes",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hierarchical" => Some(EdgeKind::Hierarchical),
            "associative" => Some(EdgeKind::Associative),
            "temporal" => Some(EdgeKind::Temporal),
            "supersedes" => Some(EdgeKind::Supersedes),
            _ => None,
        }
    }
}

/// A connection between two fragments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub source: FragmentId,
    pub target: FragmentId,
    pub kind: EdgeKind,
    pub weight: f32,
    /// Relationship description (e.g. how two associated concepts relate).
    pub content: Option<String>,
    pub created_at: i64,
}
