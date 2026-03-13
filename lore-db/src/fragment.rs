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
/// Fragments are organized as interconnected abstraction trees: higher nodes
/// capture general concepts, deeper nodes stay closer to the specifics of
/// the original conversation. Associative edges link related fragments across
/// different trees. Each node is a self-contained piece of knowledge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fragment {
    pub id: FragmentId,
    pub content: String,
    pub depth: u32,
    #[serde(skip)]
    pub embedding: Vec<f32>,
    pub created_at: i64,
    pub last_accessed: i64,
    pub access_count: u32,
    pub source_session: Option<String>,
    pub superseded_by: Option<FragmentId>,
    pub metadata: HashMap<String, String>,
    /// Intrinsic salience [0.0, 1.0]. Set at ingestion time.
    /// High = bug fixes, architectural decisions, user corrections.
    /// Low = routine observations, standard patterns.
    pub importance: f32,
    /// Pre-computed composite relevance score, updated on access and during consolidation.
    pub relevance_score: f32,
    /// Per-day exponential decay constant (lambda). Lower = slower decay.
    pub decay_rate: f32,
    /// Unix timestamp of last reinforcement event (access, consolidation touch).
    pub last_reinforced: i64,
}

impl Fragment {
    pub fn new(content: String, depth: u32) -> Self {
        let now = now_unix();
        Self {
            id: FragmentId::new(),
            content,
            depth,
            embedding: Vec::new(),
            created_at: now,
            last_accessed: now,
            access_count: 0,
            source_session: None,
            superseded_by: None,
            metadata: HashMap::new(),
            importance: 0.5,
            relevance_score: 1.0,
            decay_rate: 0.035,
            last_reinforced: now,
        }
    }

    /// Create a fragment with a specific importance level.
    /// Automatically sets the decay rate based on importance.
    pub fn new_with_importance(content: String, depth: u32, importance: f32) -> Self {
        let mut frag = Self::new(content, depth);
        frag.importance = importance.clamp(0.0, 1.0);
        frag.decay_rate = crate::relevance::decay_rate_for_importance(frag.importance);
        frag
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
