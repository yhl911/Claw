//! Long-term memory store — dreaming-inspired persistent agent memory.
//!
//! Files live under `<workspace>/.claw/memory/*.md` and are loaded into the
//! system prompt at runtime startup. Unlike `CLAUDE.md` (which is hand-edited
//! by the user), these files are written by automated dreaming consolidation
//! passes that distill patterns from session transcripts.
//!
//! Layout:
//! ```text
//! .claw/memory/
//! ├── facts.md        # stable facts about the user/project
//! ├── decisions.md    # key technical/product decisions
//! ├── patterns.md     # observed working patterns
//! ├── failures.md     # failure modes to avoid
//! └── agent_profiles/
//!     ├── opc-product.md
//!     ├── opc-engineering.md
//!     └── ...
//! ```

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Conventional set of top-level memory files. Dreaming passes target these
/// names so the system prompt assembly is deterministic.
pub const TOP_LEVEL_FILES: &[&str] =
    &["facts.md", "decisions.md", "patterns.md", "failures.md"];

#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Path-relative-to-store name, e.g. `"facts.md"` or `"agent_profiles/opc-engineering.md"`.
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    base_dir: PathBuf,
}

impl MemoryStore {
    /// Open (or implicitly create on first write) a memory store rooted at
    /// `<workspace>/.claw/memory/`. Reading from a non-existent store returns
    /// an empty list rather than erroring.
    #[must_use]
    pub fn open(workspace_root: &Path) -> Self {
        Self {
            base_dir: workspace_root.join(".claw").join("memory"),
        }
    }

    #[must_use]
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Read all `.md` files in the store (recursively, one level deep for
    /// `agent_profiles/`). Returns empty Vec if directory doesn't exist.
    pub fn read_all(&self) -> io::Result<Vec<MemoryFile>> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for name in TOP_LEVEL_FILES {
            let path = self.base_dir.join(name);
            if let Ok(content) = fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    files.push(MemoryFile {
                        name: (*name).to_string(),
                        content,
                    });
                }
            }
        }
        let profiles_dir = self.base_dir.join("agent_profiles");
        if profiles_dir.is_dir() {
            for entry in fs::read_dir(&profiles_dir)?.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                let Some(stem) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if let Ok(content) = fs::read_to_string(&path) {
                    if !content.trim().is_empty() {
                        files.push(MemoryFile {
                            name: format!("agent_profiles/{stem}"),
                            content,
                        });
                    }
                }
            }
        }
        Ok(files)
    }

    /// Atomically write a memory file. `name` may be a top-level
    /// `"facts.md"` or a nested `"agent_profiles/opc-engineering.md"`.
    pub fn write(&self, name: &str, content: &str) -> io::Result<()> {
        let target = self.base_dir.join(name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // Write to a tmp sibling, then rename for atomicity.
        let tmp = target.with_extension("md.tmp");
        fs::write(&tmp, content)?;
        fs::rename(&tmp, &target)?;
        Ok(())
    }

    /// Render all memory files into a single string suitable for appending
    /// to the system prompt. Each file becomes a `## <name>` section.
    /// Returns empty string when there is no memory.
    #[must_use]
    pub fn snapshot_for_prompt(&self) -> String {
        let Ok(files) = self.read_all() else {
            return String::new();
        };
        if files.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "## 长期记忆 (Dreaming consolidation)\n\n\
             以下是 dreaming 过程从过往会话中固化下来的知识。\
             遵循这里记录的事实/决策，避免已记录的失败模式。\n\n",
        );
        for file in files {
            let _ = write!(out, "### {}\n\n{}\n\n", file.name, file.content.trim());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn empty_store_returns_empty_snapshot() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::open(dir.path());
        assert_eq!(store.snapshot_for_prompt(), "");
        assert!(store.read_all().unwrap().is_empty());
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::open(dir.path());
        store.write("facts.md", "User prefers Rust.\n").unwrap();
        store
            .write(
                "agent_profiles/opc-engineering.md",
                "## Strengths\n- Rust refactor\n",
            )
            .unwrap();

        let files = store.read_all().unwrap();
        assert_eq!(files.len(), 2);

        let snap = store.snapshot_for_prompt();
        assert!(snap.contains("User prefers Rust."));
        assert!(snap.contains("Rust refactor"));
        assert!(snap.contains("### facts.md"));
        assert!(snap.contains("### agent_profiles/opc-engineering.md"));
    }

    #[test]
    fn empty_files_are_skipped() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::open(dir.path());
        store.write("facts.md", "  \n  ").unwrap();
        assert!(store.read_all().unwrap().is_empty());
        assert_eq!(store.snapshot_for_prompt(), "");
    }

    #[test]
    fn write_is_atomic_no_tmp_left_behind() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::open(dir.path());
        store.write("facts.md", "stable").unwrap();
        let entries: Vec<_> = std::fs::read_dir(store.base_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.contains(&"facts.md".to_string()));
        assert!(!entries.iter().any(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
        }));
    }
}
