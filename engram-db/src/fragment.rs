use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Unique identifier for a fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FragmentId(pub Uuid);

impl FragmentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }

    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl Default for FragmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FragmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A unit of knowledge — like a neuron ensemble encoding a concept.
///
/// Fragments are organized as zoom-trees where each level is a self-contained
/// summary and children are drill-downs of their parent:
/// - Depth 0: Topic overviews (rich, self-contained paragraphs)
/// - Depth 1+: Progressively more detailed drill-downs
///
/// Each node is readable standalone; children elaborate on their parent's content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fragment {
    pub id: FragmentId,
    pub content: String,
    pub summary: String,
    pub depth: u32,
    #[serde(skip)]
    pub embedding: Vec<f32>,
    pub created_at: i64,
    pub last_accessed: i64,
    pub access_count: u32,
    pub source_session: Option<String>,
    pub superseded_by: Option<FragmentId>,
    pub metadata: HashMap<String, String>,
}

impl Fragment {
    pub fn new(content: String, summary: String, depth: u32) -> Self {
        let now = now_unix();
        Self {
            id: FragmentId::new(),
            content,
            summary,
            depth,
            embedding: Vec::new(),
            created_at: now,
            last_accessed: now,
            access_count: 0,
            source_session: None,
            superseded_by: None,
            metadata: HashMap::new(),
        }
    }
}

/// A fragment with an associated relevance score from a query.
#[derive(Debug, Clone)]
pub struct ScoredFragment {
    pub fragment: Fragment,
    pub score: f32,
}

/// A tree node containing a fragment and its children.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    pub fragment: Fragment,
    pub children: Vec<Tree>,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
