use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use engram_db::Storage;

use crate::parser::{parse_jsonl_line, ConversationTurn};

/// Watches conversation log files and reads new content since last watermark.
pub struct FileWatcher {
    projects_dir: PathBuf,
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FileWatcher {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            projects_dir: PathBuf::from(home).join(".claude").join("projects"),
        }
    }

    /// Find all .jsonl conversation log files under the projects directory.
    /// Skips subagent files (mostly tool call noise).
    pub fn find_conversation_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if !self.projects_dir.exists() {
            return files;
        }
        Self::find_jsonl_recursive(&self.projects_dir, &mut files);
        // Filter out subagent files — they're mostly tool calls with little extractable knowledge
        files.retain(|f| !f.components().any(|c| c.as_os_str() == "subagents"));
        files
    }

    fn find_jsonl_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::find_jsonl_recursive(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path);
            }
        }
    }

    /// Read new conversation turns from a file starting at the watermark offset.
    /// Returns the turns and the new byte offset.
    pub fn read_new_turns(
        &self,
        file_path: &Path,
        storage: &Storage,
    ) -> Result<(Vec<ConversationTurn>, i64), Box<dyn std::error::Error>> {
        let file_str = file_path.to_string_lossy().to_string();
        let offset = storage
            .get_watermark(&file_str)?
            .map(|(off, _)| off)
            .unwrap_or(0);

        let file = std::fs::File::open(file_path)?;
        let file_len = file.metadata()?.len() as i64;

        // Nothing new
        if file_len <= offset {
            return Ok((Vec::new(), offset));
        }

        let mut reader = std::io::BufReader::new(file);
        reader.seek(SeekFrom::Start(offset as u64))?;

        let mut turns = Vec::new();
        let mut current_offset = offset;

        for line in reader.lines() {
            let line = line?;
            current_offset += line.len() as i64 + 1; // +1 for newline

            if let Some(turn) = parse_jsonl_line(&line) {
                turns.push(turn);
            }
        }

        Ok((turns, current_offset))
    }
}
