//! Skills management for the OPC desktop app.
//!
//! A "skill" is a markdown file with YAML frontmatter (`name`, `description`,
//! optional `when_to_use`) that the agent can load on demand via the existing
//! `Skill` tool. The desktop owns three operations:
//!
//! 1. **Discovery** — scan `<data>/opc-desktop/skills/<name>/SKILL.md` and
//!    return summaries to the UI (and to inject into the CEO system prompt).
//! 2. **Authoring** — create / delete / toggle local skills.
//! 3. **Import** — pull skills from a public GitHub repo (defaults to
//!    `anthropics/skills`) so users don't have to author from scratch.
//!
//! Storage layout:
//! ```text
//! <data_dir>/opc-desktop/skills/
//!   _index.json                 # { "<name>": { enabled, source, imported_at } }
//!   <skill-name>/SKILL.md       # frontmatter + body
//!   <skill-name>/<asset>...     # optional resource files
//! ```
//!
//! The existing `Skill` tool in `crates/tools` discovers skills via
//! `CLAW_CONFIG_HOME/skills`, so we point `CLAW_CONFIG_HOME` at our data
//! dir in `config::apply_config_to_env` — agents then call `Skill` with a
//! name and the tool reads the body.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// `user` (authored locally), `anthropic` (imported), or `unknown`.
    pub source: String,
    pub enabled: bool,
    pub path: String,
    /// Unix seconds.
    pub imported_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct IndexEntry {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    source: String,
    #[serde(default)]
    imported_at: u64,
}

fn default_true() -> bool {
    true
}

type Index = BTreeMap<String, IndexEntry>;

pub fn skills_root() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("opc-desktop").join("skills")
}

fn index_path() -> PathBuf {
    skills_root().join("_index.json")
}

fn load_index() -> Index {
    std::fs::read_to_string(index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(index: &Index) -> Result<(), String> {
    let path = index_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse the `description:` field from a SKILL.md YAML frontmatter block.
/// Returns empty string if no frontmatter or no description.
fn parse_description(body: &str) -> String {
    let trimmed = body.trim_start();
    if !trimmed.starts_with("---") {
        return String::new();
    }
    let after = &trimmed[3..];
    let Some(end) = after.find("\n---") else {
        return String::new();
    };
    for line in after[..end].lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("description:") {
            return rest.trim().trim_matches('"').trim_matches('\'').to_string();
        }
    }
    String::new()
}

fn sanitize_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("skill name must not be empty".into());
    }
    if trimmed.contains(['/', '\\', '.', ' ']) {
        return Err(
            "skill name must not contain '/', '\\', '.', or spaces (use kebab-case)".into(),
        );
    }
    Ok(trimmed.to_string())
}

pub fn list_skills() -> Vec<SkillInfo> {
    let root = skills_root();
    let _ = std::fs::create_dir_all(&root);
    let index = load_index();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(&skill_md).unwrap_or_default();
        let description = parse_description(&body);
        let idx = index.get(&name).cloned().unwrap_or_default();
        out.push(SkillInfo {
            name: name.clone(),
            description,
            source: if idx.source.is_empty() {
                "user".into()
            } else {
                idx.source
            },
            enabled: idx.enabled,
            path: skill_md.display().to_string(),
            imported_at: idx.imported_at,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn create_skill(name: &str, description: &str, body: &str) -> Result<SkillInfo, String> {
    let name = sanitize_name(name)?;
    let dir = skills_root().join(&name);
    if dir.exists() {
        return Err(format!("skill '{name}' already exists"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let content = format!(
        "---\nname: {name}\ndescription: {desc}\n---\n\n{body}\n",
        desc = description.replace('\n', " ").trim(),
        body = body.trim()
    );
    let skill_md = dir.join("SKILL.md");
    std::fs::write(&skill_md, content).map_err(|e| e.to_string())?;
    let mut index = load_index();
    index.insert(
        name.clone(),
        IndexEntry {
            enabled: true,
            source: "user".into(),
            imported_at: now_secs(),
        },
    );
    save_index(&index)?;
    Ok(SkillInfo {
        name: name.clone(),
        description: description.to_string(),
        source: "user".into(),
        enabled: true,
        path: skill_md.display().to_string(),
        imported_at: now_secs(),
    })
}

pub fn delete_skill(name: &str) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let dir = skills_root().join(&name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    let mut index = load_index();
    index.remove(&name);
    save_index(&index)?;
    Ok(())
}

pub fn toggle_skill(name: &str, enabled: bool) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let mut index = load_index();
    let entry = index.entry(name).or_default();
    entry.enabled = enabled;
    if entry.source.is_empty() {
        entry.source = "user".into();
    }
    save_index(&index)
}

/// Import a skill from a local folder (already on disk) into the
/// managed skills root. The source folder must contain a `SKILL.md` at
/// its top level. Files are *copied* (not moved) — original is left
/// alone. Returns the installed skill's info.
pub fn import_local_skill(src: &Path) -> Result<SkillInfo, String> {
    if !src.is_dir() {
        return Err(format!("{} is not a directory", src.display()));
    }
    let src_skill = src.join("SKILL.md");
    if !src_skill.is_file() {
        return Err(format!(
            "{} has no SKILL.md at top level",
            src.display()
        ));
    }
    let dir_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| "source path has no directory name".to_string())?;
    let name = sanitize_name(&dir_name)?;
    let dest = skills_root().join(&name);
    if dest.exists() {
        return Err(format!("skill '{name}' already exists locally"));
    }
    copy_dir_recursive(src, &dest).map_err(|e| {
        let _ = std::fs::remove_dir_all(&dest);
        e
    })?;

    let skill_md = dest.join("SKILL.md");
    let body = std::fs::read_to_string(&skill_md).unwrap_or_default();
    let description = parse_description(&body);
    let mut index = load_index();
    index.insert(
        name.clone(),
        IndexEntry {
            enabled: true,
            source: "user".into(),
            imported_at: now_secs(),
        },
    );
    save_index(&index)?;

    Ok(SkillInfo {
        name: name.clone(),
        description,
        source: "user".into(),
        enabled: true,
        path: skill_md.display().to_string(),
        imported_at: now_secs(),
    })
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if from.is_file() {
            std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn read_skill(name: &str) -> Result<String, String> {
    let name = sanitize_name(name)?;
    let path = skills_root().join(&name).join("SKILL.md");
    std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))
}

/// One entry returned by `list_remote_skills`. The `path` is the repo
/// path of the skill root (the directory containing SKILL.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSkill {
    pub name: String,
    pub path: String,
    pub kind: String,
}

const DEFAULT_REPO: &str = "anthropics/skills";
const TARBALL_TTL_SECS: u64 = 60 * 60; // refresh after 1h

/// One downloaded repo tarball, kept in-process so list + multiple imports
/// share a single network fetch. Memory-only by design — small (a few MB),
/// rebuilt cheaply on app restart.
struct TarballCache {
    repo: String,
    branch: String,
    bytes: Vec<u8>,
    fetched_at: u64,
}

fn cache() -> &'static Mutex<Option<TarballCache>> {
    static CACHE: Mutex<Option<TarballCache>> = Mutex::new(None);
    &CACHE
}

fn build_client() -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent("opc-desktop/0.1")
        // No overall timeout — repos like anthropics/skills can be 3MB+
        // and download speed from codeload varies wildly by region (we've
        // seen 20KB/s, which would need 3+ minutes). An overall timeout
        // here surfaces in reqwest as "error decoding response body",
        // which is misleading. Instead enforce only a connect-phase
        // timeout — once bytes start flowing, let it run to completion.
        .connect_timeout(std::time::Duration::from_secs(30));
    // Optional bearer auth so codeload accepts requests against private
    // forks the user has configured; also avoids the unauthenticated
    // throttling some IP ranges hit on codeload.
    if let Some(token) = github_token() {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, v);
            builder = builder.default_headers(headers);
        }
    }
    builder.build().map_err(|e| e.to_string())
}

/// Read the user's configured GitHub token from environment. The desktop
/// app mirrors the setting (`config.github_token`) into `OPC_GITHUB_TOKEN`
/// at startup so this lookup works in both the main worker and the daemon.
/// Also accepts the common `GITHUB_TOKEN`/`GH_TOKEN` aliases.
fn github_token() -> Option<String> {
    for key in ["OPC_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(key) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Download (or return cached) repo tarball as bytes. Tries `main` then
/// `master` from codeload.github.com — this endpoint is CDN-served and not
/// subject to the api.github.com 60-req/hr rate limit.
async fn fetch_tarball(repo: &str) -> Result<(String, Vec<u8>), String> {
    {
        let guard = cache().lock().map_err(|_| "tarball cache poisoned".to_string())?;
        if let Some(c) = guard.as_ref() {
            if c.repo == repo && now_secs().saturating_sub(c.fetched_at) < TARBALL_TTL_SECS {
                return Ok((c.branch.clone(), c.bytes.clone()));
            }
        }
    }
    let client = build_client()?;
    let mut last_err = String::new();
    for branch in ["main", "master"] {
        let url = format!("https://codeload.github.com/{repo}/tar.gz/refs/heads/{branch}");
        eprintln!("[skills] fetching tarball: {url}");
        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("fetch {url}: {e}");
                continue;
            }
        };
        let status = resp.status();
        if !status.is_success() {
            last_err = format!("{status} {url}");
            continue;
        }
        // Stream the body chunk-by-chunk rather than `.bytes().await`. On
        // big responses with chunked transfer encoding the latter can fail
        // with "error decoding response body" if any single chunk read
        // returns an unexpected length; manual collection is more
        // forgiving and lets us surface partial-read errors clearly.
        use futures_util::StreamExt;
        let total: Option<u64> = resp.content_length();
        if let Some(t) = total {
            eprintln!("[skills] expected size: {t} bytes");
        }
        let mut stream = resp.bytes_stream();
        let mut bytes: Vec<u8> = Vec::new();
        let mut chunk_err: Option<String> = None;
        let mut last_log = 0usize;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(b) => {
                    bytes.extend_from_slice(&b);
                    // Log every ~256KB so the user sees progress in the
                    // terminal during slow downloads.
                    if bytes.len() - last_log >= 256 * 1024 {
                        last_log = bytes.len();
                        eprintln!(
                            "[skills] downloaded {} / {} bytes",
                            bytes.len(),
                            total.map_or("?".to_string(), |t| t.to_string())
                        );
                    }
                }
                Err(e) => {
                    chunk_err = Some(format!(
                        "stream broken after {} bytes (network issue / connection reset): {e}",
                        bytes.len()
                    ));
                    break;
                }
            }
        }
        if let Some(e) = chunk_err {
            last_err = e;
            continue;
        }
        if let Some(t) = total {
            if (bytes.len() as u64) < t {
                last_err = format!(
                    "incomplete download: got {} of {t} bytes",
                    bytes.len()
                );
                continue;
            }
        }
        eprintln!("[skills] tarball complete: {} bytes", bytes.len());
        if let Ok(mut guard) = cache().lock() {
            *guard = Some(TarballCache {
                repo: repo.to_string(),
                branch: branch.to_string(),
                bytes: bytes.clone(),
                fetched_at: now_secs(),
            });
        }
        return Ok((branch.to_string(), bytes));
    }
    Err(format!(
        "could not download tarball for {repo} (tried main, master): {last_err}"
    ))
}

/// One entry in the in-memory tarball — content plus its path *relative*
/// to the repo root (with the GitHub-prefixed `repo-sha/` segment stripped).
struct TarEntry {
    rel_path: String,
    content: Vec<u8>,
}

fn extract_tarball(bytes: &[u8]) -> Result<Vec<TarEntry>, String> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut out = Vec::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        // codeload tarballs always have a top-level `{repo}-{sha}/` segment.
        let mut comps = path.components();
        comps.next();
        let rel = comps.as_path().to_string_lossy().to_string();
        if rel.is_empty() {
            continue;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        out.push(TarEntry {
            rel_path: rel,
            content: buf,
        });
    }
    Ok(out)
}

/// List skills available in a public GitHub repository.
/// Strategy: download (or reuse cached) tarball via codeload CDN, scan for
/// every `**/SKILL.md`, return its parent dir as one skill entry.
pub async fn list_remote_skills(repo: Option<String>) -> Result<Vec<RemoteSkill>, String> {
    let repo = repo
        .map(|r| r.trim().trim_matches('/').to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    let (_branch, bytes) = fetch_tarball(&repo).await?;
    let entries = extract_tarball(&bytes)?;

    let mut out: Vec<RemoteSkill> = entries
        .into_iter()
        .filter_map(|e| {
            let lower = e.rel_path.to_ascii_lowercase();
            if !lower.ends_with("/skill.md") {
                return None;
            }
            let dir = Path::new(&e.rel_path).parent()?.to_string_lossy().to_string();
            let name = Path::new(&dir).file_name()?.to_string_lossy().to_string();
            Some(RemoteSkill {
                name,
                path: dir,
                kind: "dir".into(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

/// Install one skill from a public GitHub repository.
/// Strategy: ensure the repo tarball is cached, then extract only files
/// under the requested skill path into the local skills root.
pub async fn import_remote_skill(
    repo: Option<String>,
    path: &str,
) -> Result<SkillInfo, String> {
    let repo = repo
        .map(|r| r.trim().trim_matches('/').to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());

    let dir_name = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| "remote path has no file name".to_string())?;
    let name = sanitize_name(&dir_name)?;
    let dest = skills_root().join(&name);
    if dest.exists() {
        return Err(format!("skill '{name}' already exists locally"));
    }

    let (_branch, bytes) = fetch_tarball(&repo).await?;
    let entries = extract_tarball(&bytes)?;
    let prefix = format!("{}/", path.trim_end_matches('/'));
    let matched: Vec<TarEntry> = entries
        .into_iter()
        .filter(|e| e.rel_path.starts_with(&prefix))
        .collect();
    if matched.is_empty() {
        return Err(format!("no files found under '{path}' in {repo}"));
    }

    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let install = || -> Result<(), String> {
        for e in &matched {
            let rel = e.rel_path.strip_prefix(&prefix).unwrap_or(&e.rel_path);
            let local = dest.join(rel);
            if let Some(parent) = local.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            std::fs::write(&local, &e.content).map_err(|err| err.to_string())?;
        }
        Ok(())
    };
    if let Err(e) = install() {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e);
    }

    let skill_md = dest.join("SKILL.md");
    if !skill_md.is_file() {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(format!("imported '{name}' has no SKILL.md at top level"));
    }
    let body = std::fs::read_to_string(&skill_md).unwrap_or_default();
    let description = parse_description(&body);
    let mut index = load_index();
    index.insert(
        name.clone(),
        IndexEntry {
            enabled: true,
            source: "anthropic".into(),
            imported_at: now_secs(),
        },
    );
    save_index(&index)?;

    Ok(SkillInfo {
        name: name.clone(),
        description,
        source: "anthropic".into(),
        enabled: true,
        path: skill_md.display().to_string(),
        imported_at: now_secs(),
    })
}

/// Build a short markdown summary of all enabled skills, suitable for
/// appending to the agent's system prompt. Returns empty string if no
/// skills are enabled — caller should skip the push in that case.
pub fn enabled_skills_prompt_section() -> String {
    let skills: Vec<SkillInfo> = list_skills().into_iter().filter(|s| s.enabled).collect();
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## 可用 Skills（按需加载）\n\n\
        当任务匹配以下 skill 的描述时，调用 `Skill` 工具加载完整说明再执行。\
        未列出的能力不要假设存在。\n\n",
    );
    for s in skills {
        let desc = if s.description.is_empty() {
            "(no description)"
        } else {
            &s.description
        };
        out.push_str(&format!("- `{}` — {}\n", s.name, desc));
    }
    out.push_str("\n用法：`Skill({ skill: \"<name>\" })` 返回完整 SKILL.md 内容。\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_description_extracts_field() {
        let body = "---\nname: foo\ndescription: A test skill\n---\n\nBody";
        assert_eq!(parse_description(body), "A test skill");
    }

    #[test]
    fn parse_description_handles_missing_frontmatter() {
        assert_eq!(parse_description("no frontmatter here"), "");
    }

    #[test]
    fn sanitize_name_rejects_slashes() {
        assert!(sanitize_name("foo/bar").is_err());
        assert!(sanitize_name("foo bar").is_err());
        assert!(sanitize_name("foo.bar").is_err());
        assert!(sanitize_name("foo-bar").is_ok());
    }
}
