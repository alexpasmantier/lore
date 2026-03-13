use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

use crate::edge::{Edge, EdgeId, EdgeKind};
use crate::fragment::{now_unix, Fragment, FragmentId};

/// SQLite-backed storage for the lore knowledge graph.
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Open (or create) a database at the given path with WAL mode.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    /// Open a database in read-only mode (for the MCP server).
    pub fn open_readonly(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    /// Run database migrations.
    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS fragments (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                summary TEXT NOT NULL,
                depth INTEGER NOT NULL,
                embedding BLOB,
                created_at INTEGER NOT NULL,
                last_accessed INTEGER NOT NULL,
                access_count INTEGER DEFAULT 0,
                source_session TEXT,
                superseded_by TEXT REFERENCES fragments(id),
                metadata TEXT,
                importance REAL DEFAULT 0.5,
                relevance_score REAL DEFAULT 1.0,
                decay_rate REAL DEFAULT 0.035,
                last_reinforced INTEGER
            );

            CREATE TABLE IF NOT EXISTS edges (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL REFERENCES fragments(id),
                target TEXT NOT NULL REFERENCES fragments(id),
                kind TEXT NOT NULL,
                weight REAL DEFAULT 1.0,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS watermarks (
                file_path TEXT PRIMARY KEY,
                byte_offset INTEGER NOT NULL,
                last_processed INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_fragments_depth ON fragments(depth);
            CREATE INDEX IF NOT EXISTS idx_fragments_superseded ON fragments(superseded_by)
                WHERE superseded_by IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source);
            CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target);
            CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);

            CREATE TABLE IF NOT EXISTS staged_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL,
                role TEXT NOT NULL,
                text TEXT NOT NULL,
                staged_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_staged_file ON staged_turns(file_path);
            ",
        )?;

        // V2 migration: add new columns to existing databases
        // Must run before creating indexes on V2 columns
        self.migrate_v2()?;

        // Create index on V2 column (safe to run after migrate_v2)
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_fragments_relevance
                ON fragments(relevance_score) WHERE superseded_by IS NULL;",
        )?;

        Ok(())
    }

    /// V2 migration: Add relevance/importance columns if they don't exist.
    fn migrate_v2(&self) -> rusqlite::Result<()> {
        // Check if the columns already exist by trying a query
        let has_importance = self
            .conn
            .prepare("SELECT importance FROM fragments LIMIT 0")
            .is_ok();

        if !has_importance {
            self.conn.execute_batch(
                "
                ALTER TABLE fragments ADD COLUMN importance REAL DEFAULT 0.5;
                ALTER TABLE fragments ADD COLUMN relevance_score REAL DEFAULT 1.0;
                ALTER TABLE fragments ADD COLUMN decay_rate REAL DEFAULT 0.035;
                ALTER TABLE fragments ADD COLUMN last_reinforced INTEGER;
                CREATE INDEX IF NOT EXISTS idx_fragments_relevance
                    ON fragments(relevance_score) WHERE superseded_by IS NULL;
                ",
            )?;
        }

        Ok(())
    }

    // ──── Fragment CRUD ────

    /// Insert a fragment into the database.
    pub fn insert_fragment(&self, fragment: &Fragment) -> rusqlite::Result<()> {
        let embedding_blob = if fragment.embedding.is_empty() {
            None
        } else {
            Some(embedding_to_bytes(&fragment.embedding))
        };
        let metadata_json = serde_json::to_string(&fragment.metadata).unwrap_or_default();
        let superseded_by = fragment.superseded_by.map(|id| id.as_str());

        self.conn.execute(
            "INSERT INTO fragments (id, content, summary, depth, embedding, created_at,
             last_accessed, access_count, source_session, superseded_by, metadata,
             importance, relevance_score, decay_rate, last_reinforced)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                fragment.id.as_str(),
                fragment.content,
                fragment.summary,
                fragment.depth,
                embedding_blob,
                fragment.created_at,
                fragment.last_accessed,
                fragment.access_count,
                fragment.source_session,
                superseded_by,
                metadata_json,
                fragment.importance,
                fragment.relevance_score,
                fragment.decay_rate,
                fragment.last_reinforced,
            ],
        )?;
        Ok(())
    }

    /// Get a fragment by ID.
    pub fn get_fragment(&self, id: FragmentId) -> rusqlite::Result<Option<Fragment>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM fragments WHERE id = ?1",
            FRAGMENT_COLUMNS
        ))?;

        let mut rows = stmt.query_map(params![id.as_str()], row_to_fragment)?;
        match rows.next() {
            Some(Ok(f)) => Ok(Some(f)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Get all fragments at a specific depth.
    pub fn get_fragments_at_depth(&self, depth: u32) -> rusqlite::Result<Vec<Fragment>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM fragments WHERE depth = ?1 AND superseded_by IS NULL",
            FRAGMENT_COLUMNS
        ))?;

        let fragments = stmt
            .query_map(params![depth], row_to_fragment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(fragments)
    }

    /// Update last_accessed and increment access_count for a fragment.
    pub fn touch_fragment(&self, id: FragmentId) -> rusqlite::Result<()> {
        let now = now_unix();
        self.conn.execute(
            "UPDATE fragments SET last_accessed = ?1, access_count = access_count + 1
             WHERE id = ?2",
            params![now, id.as_str()],
        )?;
        Ok(())
    }

    /// Reinforce a fragment: update access tracking AND relevance score.
    /// This is the reconsolidation-on-recall mechanism.
    pub fn reinforce_fragment(
        &self,
        id: FragmentId,
        now: i64,
        new_relevance: f32,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE fragments SET
                last_accessed = ?1,
                access_count = access_count + 1,
                last_reinforced = ?1,
                relevance_score = ?2
             WHERE id = ?3",
            params![now, new_relevance, id.as_str()],
        )?;
        Ok(())
    }

    /// Boost a fragment's relevance score by a small delta (spreading activation).
    /// Does NOT reset last_reinforced — this is a passive boost from neighbor access.
    pub fn boost_relevance(&self, id: FragmentId, boost: f32, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE fragments SET
                relevance_score = MIN(relevance_score + ?1, 1.0),
                last_accessed = ?2
             WHERE id = ?3 AND superseded_by IS NULL",
            params![boost, now, id.as_str()],
        )?;
        Ok(())
    }

    /// Recompute relevance scores for all active fragments.
    /// This is the "sleep cycle" decay pass — called during consolidation.
    pub fn recompute_all_relevance(&self, now: i64) -> rusqlite::Result<usize> {
        use crate::relevance::compute_relevance;

        let mut stmt = self.conn.prepare(
            "SELECT id, importance, access_count, decay_rate, last_reinforced, created_at
             FROM fragments WHERE superseded_by IS NULL",
        )?;

        let rows: Vec<(String, f32, u32, f32, Option<i64>, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f32>(1).unwrap_or(0.5),
                    row.get::<_, u32>(2).unwrap_or(0),
                    row.get::<_, f32>(3).unwrap_or(0.035),
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut update_stmt = self
            .conn
            .prepare("UPDATE fragments SET relevance_score = ?1 WHERE id = ?2")?;

        let mut updated = 0;
        for (id, importance, access_count, decay_rate, last_reinforced, created_at) in &rows {
            let reinforced = last_reinforced.unwrap_or(*created_at);
            let relevance =
                compute_relevance(*importance, *access_count, *decay_rate, reinforced, now);
            update_stmt.execute(params![relevance, id])?;
            updated += 1;
        }

        Ok(updated)
    }

    /// Decay all edge weights of a given kind by a multiplicative factor.
    pub fn decay_edge_weights(&self, kind: EdgeKind, factor: f32) -> rusqlite::Result<usize> {
        let affected = self.conn.execute(
            "UPDATE edges SET weight = weight * ?1 WHERE kind = ?2",
            params![factor, kind.as_str()],
        )?;
        Ok(affected)
    }

    /// Get fragments with relevance below a threshold for pruning.
    pub fn get_low_relevance_fragments(
        &self,
        max_relevance: f32,
        min_age_secs: i64,
        now: i64,
    ) -> rusqlite::Result<Vec<Fragment>> {
        let cutoff = now - min_age_secs;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM fragments
                 WHERE superseded_by IS NULL
                 AND relevance_score < ?1
                 AND created_at < ?2
                 AND depth > 0",
            FRAGMENT_COLUMNS,
        ))?;

        let fragments = stmt
            .query_map(params![max_relevance, cutoff], row_to_fragment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(fragments)
    }

    /// Update content, summary, and embedding of an existing fragment in-place.
    pub fn update_fragment_content(
        &self,
        id: FragmentId,
        new_content: &str,
        new_summary: &str,
        new_embedding: Option<&[f32]>,
    ) -> rusqlite::Result<()> {
        if let Some(emb) = new_embedding {
            let blob = embedding_to_bytes(emb);
            self.conn.execute(
                "UPDATE fragments SET content = ?1, summary = ?2, embedding = ?3, last_accessed = ?4 WHERE id = ?5",
                params![new_content, new_summary, blob, now_unix(), id.as_str()],
            )?;
        } else {
            self.conn.execute(
                "UPDATE fragments SET content = ?1, summary = ?2, last_accessed = ?3 WHERE id = ?4",
                params![new_content, new_summary, now_unix(), id.as_str()],
            )?;
        }
        Ok(())
    }

    /// Mark a fragment as superseded by another.
    pub fn supersede_fragment(&self, old: FragmentId, new: FragmentId) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE fragments SET superseded_by = ?1 WHERE id = ?2",
            params![new.as_str(), old.as_str()],
        )?;
        Ok(())
    }

    /// Delete a fragment and all its edges.
    pub fn delete_fragment(&self, id: FragmentId) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM edges WHERE source = ?1 OR target = ?1",
            params![id.as_str()],
        )?;
        self.conn
            .execute("DELETE FROM fragments WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    // ──── Edge CRUD ────

    /// Insert an edge between two fragments.
    pub fn insert_edge(&self, edge: &Edge) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO edges (id, source, target, kind, weight, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                edge.id.to_string(),
                edge.source.as_str(),
                edge.target.as_str(),
                edge.kind.as_str(),
                edge.weight,
                edge.created_at,
            ],
        )?;
        Ok(())
    }

    /// Get all children of a fragment (via hierarchical edges where this fragment is source).
    pub fn get_children(&self, id: FragmentId) -> rusqlite::Result<Vec<Fragment>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM fragments f
                 INNER JOIN edges e ON e.target = f.id
                 WHERE e.source = ?1 AND e.kind = 'hierarchical'
                 AND f.superseded_by IS NULL",
            FRAGMENT_COLUMNS_PREFIXED,
        ))?;

        let fragments = stmt
            .query_map(params![id.as_str()], row_to_fragment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(fragments)
    }

    /// Get the parent of a fragment (via hierarchical edge where this fragment is target).
    pub fn get_parent(&self, id: FragmentId) -> rusqlite::Result<Option<Fragment>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM fragments f
                 INNER JOIN edges e ON e.source = f.id
                 WHERE e.target = ?1 AND e.kind = 'hierarchical'",
            FRAGMENT_COLUMNS_PREFIXED,
        ))?;

        let mut rows = stmt.query_map(params![id.as_str()], row_to_fragment)?;
        match rows.next() {
            Some(Ok(f)) => Ok(Some(f)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Get fragments connected by associative edges.
    pub fn get_associations(&self, id: FragmentId) -> rusqlite::Result<Vec<Fragment>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM fragments f
                 INNER JOIN edges e ON (e.target = f.id OR e.source = f.id)
                 WHERE (e.source = ?1 OR e.target = ?1) AND e.kind = 'associative'
                 AND f.id != ?1 AND f.superseded_by IS NULL",
            FRAGMENT_COLUMNS_PREFIXED,
        ))?;

        let fragments = stmt
            .query_map(params![id.as_str()], row_to_fragment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(fragments)
    }

    /// Get all edges from or to a fragment.
    pub fn get_edges_for(&self, id: FragmentId) -> rusqlite::Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, target, kind, weight, created_at
             FROM edges WHERE source = ?1 OR target = ?1",
        )?;

        let edges = stmt
            .query_map(params![id.as_str()], row_to_edge)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(edges)
    }

    /// Update edge weight.
    pub fn update_edge_weight(&self, id: EdgeId, weight: f32) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE edges SET weight = ?1 WHERE id = ?2",
            params![weight, id.to_string()],
        )?;
        Ok(())
    }

    /// Delete edges below a weight threshold for a given kind.
    pub fn delete_weak_edges(&self, kind: EdgeKind, min_weight: f32) -> rusqlite::Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM edges WHERE kind = ?1 AND weight < ?2",
            params![kind.as_str(), min_weight],
        )?;
        Ok(deleted)
    }

    /// Delete an edge between two specific fragments of a given kind.
    pub fn delete_edge_between(
        &self,
        source: FragmentId,
        target: FragmentId,
        kind: EdgeKind,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM edges WHERE source = ?1 AND target = ?2 AND kind = ?3",
            params![source.as_str(), target.as_str(), kind.as_str()],
        )?;
        Ok(())
    }

    // ──── Semantic Search ────

    /// Load all fragments at a given depth that have embeddings.
    /// Returns them for in-memory similarity computation.
    pub fn get_fragments_with_embeddings(
        &self,
        depth: Option<u32>,
    ) -> rusqlite::Result<Vec<Fragment>> {
        let base = format!(
            "SELECT {} FROM fragments WHERE embedding IS NOT NULL AND superseded_by IS NULL",
            FRAGMENT_COLUMNS,
        );
        let sql = match depth {
            Some(_) => format!("{} AND depth = ?1", base),
            None => base,
        };

        let mut stmt = self.conn.prepare(&sql)?;

        let fragments = if let Some(d) = depth {
            stmt.query_map(params![d], row_to_fragment)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map([], row_to_fragment)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        Ok(fragments)
    }

    // ──── Watermarks ────

    /// Get the watermark for a file path.
    pub fn get_watermark(&self, file_path: &str) -> rusqlite::Result<Option<(i64, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT byte_offset, last_processed FROM watermarks WHERE file_path = ?1")?;

        let mut rows = stmt.query_map(params![file_path], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;

        match rows.next() {
            Some(Ok(wm)) => Ok(Some(wm)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Set the watermark for a file path.
    pub fn set_watermark(&self, file_path: &str, byte_offset: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO watermarks (file_path, byte_offset, last_processed)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(file_path) DO UPDATE SET byte_offset = ?2, last_processed = ?3",
            params![file_path, byte_offset, now_unix()],
        )?;
        Ok(())
    }

    // ──── Staged turns ────

    /// Insert raw conversation turns into the staging table.
    pub fn stage_turns(&self, file_path: &str, turns: &[(&str, &str)]) -> rusqlite::Result<usize> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        for (role, text) in turns {
            tx.execute(
                "INSERT INTO staged_turns (file_path, role, text, staged_at) VALUES (?1, ?2, ?3, ?4)",
                params![file_path, role, text, now],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    /// Return sessions whose most recent staged turn is older than the idle threshold.
    pub fn get_staged_sessions(
        &self,
        idle_threshold_secs: i64,
        now: i64,
    ) -> rusqlite::Result<Vec<StagedSession>> {
        let cutoff = now - idle_threshold_secs;
        let mut stmt = self.conn.prepare(
            "SELECT file_path, MAX(staged_at) as last_staged, COUNT(*) as turn_count
             FROM staged_turns
             GROUP BY file_path
             HAVING MAX(staged_at) < ?1
             ORDER BY last_staged ASC",
        )?;
        let sessions = stmt
            .query_map(params![cutoff], |row| {
                Ok(StagedSession {
                    file_path: row.get(0)?,
                    last_staged: row.get(1)?,
                    turn_count: row.get::<_, usize>(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(sessions)
    }

    /// Get all staged turns for a session, ordered by insertion.
    pub fn get_staged_turns(&self, file_path: &str) -> rusqlite::Result<Vec<StagedTurn>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, text FROM staged_turns WHERE file_path = ?1 ORDER BY id ASC",
        )?;
        let turns = stmt
            .query_map(params![file_path], |row| {
                Ok(StagedTurn {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    text: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(turns)
    }

    /// Delete all staged turns for a session after digestion.
    pub fn delete_staged_turns(&self, file_path: &str) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM staged_turns WHERE file_path = ?1", params![file_path])
    }

    /// Get a reference to the underlying connection (for transactions).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// A conversation session with staged turns awaiting digestion.
pub struct StagedSession {
    pub file_path: String,
    pub last_staged: i64,
    pub turn_count: usize,
}

/// A single staged conversation turn.
pub struct StagedTurn {
    pub id: i64,
    pub role: String,
    pub text: String,
}

// ──── Helper functions ────

/// Column list for fragment queries (no table prefix).
const FRAGMENT_COLUMNS: &str =
    "id, content, summary, depth, embedding, created_at, last_accessed, \
     access_count, source_session, superseded_by, metadata, \
     importance, relevance_score, decay_rate, last_reinforced";

/// Column list for fragment queries (with f. table prefix for JOINs).
const FRAGMENT_COLUMNS_PREFIXED: &str =
    "f.id, f.content, f.summary, f.depth, f.embedding, f.created_at, f.last_accessed, \
     f.access_count, f.source_session, f.superseded_by, f.metadata, \
     f.importance, f.relevance_score, f.decay_rate, f.last_reinforced";

fn row_to_fragment(row: &rusqlite::Row) -> rusqlite::Result<Fragment> {
    let id_str: String = row.get(0)?;
    let embedding_blob: Option<Vec<u8>> = row.get(4)?;
    let superseded_str: Option<String> = row.get(9)?;
    let metadata_str: Option<String> = row.get(10)?;

    let embedding = embedding_blob
        .map(|b| bytes_to_embedding(&b))
        .unwrap_or_default();

    let superseded_by = superseded_str.and_then(|s| FragmentId::parse(&s).ok());

    let metadata: HashMap<String, String> = metadata_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let created_at: i64 = row.get(5)?;
    let last_reinforced: Option<i64> = row.get(14).unwrap_or(None);

    Ok(Fragment {
        id: FragmentId::parse(&id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        content: row.get(1)?,
        summary: row.get(2)?,
        depth: row.get::<_, u32>(3)?,
        embedding,
        created_at,
        last_accessed: row.get(6)?,
        access_count: row.get::<_, u32>(7)?,
        source_session: row.get(8)?,
        superseded_by,
        metadata,
        importance: row.get::<_, f32>(11).unwrap_or(0.5),
        relevance_score: row.get::<_, f32>(12).unwrap_or(1.0),
        decay_rate: row.get::<_, f32>(13).unwrap_or(0.035),
        last_reinforced: last_reinforced.unwrap_or(created_at),
    })
}

fn row_to_edge(row: &rusqlite::Row) -> rusqlite::Result<Edge> {
    let id_str: String = row.get(0)?;
    let source_str: String = row.get(1)?;
    let target_str: String = row.get(2)?;
    let kind_str: String = row.get(3)?;

    Ok(Edge {
        id: EdgeId(uuid::Uuid::parse_str(&id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?),
        source: FragmentId::parse(&source_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?,
        target: FragmentId::parse(&target_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?,
        kind: EdgeKind::parse(&kind_str).unwrap_or(EdgeKind::Associative),
        weight: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// Convert f32 embedding vector to bytes for SQLite BLOB storage.
fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for &val in embedding {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Convert bytes from SQLite BLOB back to f32 embedding vector.
fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedding_roundtrip() {
        let original = vec![1.0f32, -0.5, 0.0, 0.75, -1.25];
        let bytes = embedding_to_bytes(&original);
        let recovered = bytes_to_embedding(&bytes);
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_fragment_crud() {
        let storage = Storage::open_memory().unwrap();

        let mut frag = Fragment::new(
            "Rust async programming".to_string(),
            "Async Rust".to_string(),
            0,
        );
        frag.embedding = vec![1.0, 2.0, 3.0];

        storage.insert_fragment(&frag).unwrap();

        let loaded = storage.get_fragment(frag.id).unwrap().unwrap();
        assert_eq!(loaded.content, "Rust async programming");
        assert_eq!(loaded.summary, "Async Rust");
        assert_eq!(loaded.depth, 0);
        assert_eq!(loaded.embedding, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_edge_crud() {
        let storage = Storage::open_memory().unwrap();

        let parent = Fragment::new("Topic".to_string(), "Topic".to_string(), 0);
        let child = Fragment::new("Concept".to_string(), "Concept".to_string(), 1);

        storage.insert_fragment(&parent).unwrap();
        storage.insert_fragment(&child).unwrap();

        let edge = Edge {
            id: EdgeId::new(),
            source: parent.id,
            target: child.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            created_at: now_unix(),
        };
        storage.insert_edge(&edge).unwrap();

        let children = storage.get_children(parent.id).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, child.id);

        let loaded_parent = storage.get_parent(child.id).unwrap().unwrap();
        assert_eq!(loaded_parent.id, parent.id);
    }

    #[test]
    fn test_update_fragment_content() {
        let storage = Storage::open_memory().unwrap();

        let mut frag = Fragment::new(
            "Original content".to_string(),
            "Original summary".to_string(),
            0,
        );
        frag.embedding = vec![1.0, 2.0, 3.0];
        storage.insert_fragment(&frag).unwrap();

        // Update with new embedding
        let new_emb = vec![4.0, 5.0, 6.0];
        storage
            .update_fragment_content(
                frag.id,
                "Updated content",
                "Updated summary",
                Some(&new_emb),
            )
            .unwrap();

        let loaded = storage.get_fragment(frag.id).unwrap().unwrap();
        assert_eq!(loaded.content, "Updated content");
        assert_eq!(loaded.summary, "Updated summary");
        assert_eq!(loaded.embedding, vec![4.0, 5.0, 6.0]);

        // Update without changing embedding
        storage
            .update_fragment_content(frag.id, "Content v3", "Summary v3", None)
            .unwrap();

        let loaded = storage.get_fragment(frag.id).unwrap().unwrap();
        assert_eq!(loaded.content, "Content v3");
        assert_eq!(loaded.summary, "Summary v3");
        assert_eq!(loaded.embedding, vec![4.0, 5.0, 6.0]); // unchanged
    }

    #[test]
    fn test_delete_edge_between() {
        let storage = Storage::open_memory().unwrap();

        let parent = Fragment::new("Topic".to_string(), "Topic".to_string(), 0);
        let child = Fragment::new("Concept".to_string(), "Concept".to_string(), 1);
        storage.insert_fragment(&parent).unwrap();
        storage.insert_fragment(&child).unwrap();

        let edge = Edge {
            id: EdgeId::new(),
            source: parent.id,
            target: child.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            created_at: now_unix(),
        };
        storage.insert_edge(&edge).unwrap();

        assert_eq!(storage.get_children(parent.id).unwrap().len(), 1);

        storage
            .delete_edge_between(parent.id, child.id, EdgeKind::Hierarchical)
            .unwrap();

        assert_eq!(storage.get_children(parent.id).unwrap().len(), 0);
    }

    #[test]
    fn test_watermark() {
        let storage = Storage::open_memory().unwrap();

        assert!(storage.get_watermark("/some/file.jsonl").unwrap().is_none());

        storage.set_watermark("/some/file.jsonl", 1024).unwrap();
        let (offset, _) = storage.get_watermark("/some/file.jsonl").unwrap().unwrap();
        assert_eq!(offset, 1024);

        storage.set_watermark("/some/file.jsonl", 2048).unwrap();
        let (offset, _) = storage.get_watermark("/some/file.jsonl").unwrap().unwrap();
        assert_eq!(offset, 2048);
    }

    #[test]
    fn test_stage_and_retrieve_turns() {
        let storage = Storage::open_memory().unwrap();

        let turns = vec![
            ("user", "Hello"),
            ("assistant", "Hi there"),
            ("user", "How does X work?"),
        ];
        let count = storage.stage_turns("/path/to/session.jsonl", &turns).unwrap();
        assert_eq!(count, 3);

        let retrieved = storage.get_staged_turns("/path/to/session.jsonl").unwrap();
        assert_eq!(retrieved.len(), 3);
        assert_eq!(retrieved[0].role, "user");
        assert_eq!(retrieved[0].text, "Hello");
        assert_eq!(retrieved[1].role, "assistant");
        assert_eq!(retrieved[2].text, "How does X work?");
    }

    #[test]
    fn test_staged_sessions_idle_threshold() {
        let storage = Storage::open_memory().unwrap();
        let now = now_unix();

        // Stage turns for two sessions
        storage.stage_turns("/idle.jsonl", &[("user", "old")]).unwrap();
        storage.stage_turns("/active.jsonl", &[("user", "fresh")]).unwrap();

        // Backdate the idle session
        storage.conn().execute(
            "UPDATE staged_turns SET staged_at = ?1 WHERE file_path = ?2",
            params![now - 600, "/idle.jsonl"],
        ).unwrap();

        // With 300s threshold, only the idle session should be returned
        let sessions = storage.get_staged_sessions(300, now).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].file_path, "/idle.jsonl");
        assert_eq!(sessions[0].turn_count, 1);
    }

    #[test]
    fn test_delete_staged_turns() {
        let storage = Storage::open_memory().unwrap();

        storage.stage_turns("/session.jsonl", &[("user", "hello"), ("assistant", "hi")]).unwrap();
        assert_eq!(storage.get_staged_turns("/session.jsonl").unwrap().len(), 2);

        let deleted = storage.delete_staged_turns("/session.jsonl").unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(storage.get_staged_turns("/session.jsonl").unwrap().len(), 0);
    }
}
