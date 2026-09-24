use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub(crate) const MAX_CLAUDE_SESSIONS: usize = 30;
pub(crate) const CLAUDE_SESSION_SCAN_INTERVAL: Duration = Duration::from_secs(5);
const MAX_TITLE_CHARS: usize = 120;
const READ_CHUNK_BYTES: usize = 256 * 1024;
pub(crate) const MAX_SESSION_REPOS: usize = 20;
const MAX_COMMAND_PATHS: usize = 16;
const MAX_REPO_CACHE: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeSession {
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    pub context: String,
    pub worktree_path: Option<String>,
    pub repos: Vec<String>,
    pub updated_at_ms: u64,
}

pub(crate) type ClaudeSessionIndex = Arc<Mutex<Vec<ClaudeSession>>>;

#[derive(Debug, Default)]
struct TranscriptState {
    offset: u64,
    modified: Option<SystemTime>,
    session_id: Option<String>,
    cwd: Option<String>,
    custom_title: Option<String>,
    agent_name: Option<String>,
    first_prompt: Option<String>,
    tool_worktree: Option<String>,
    tool_worktree_path: Option<String>,
    repos: Vec<String>,
}

impl TranscriptState {
    fn title(&self) -> Option<&str> {
        self.custom_title
            .as_deref()
            .or(self.agent_name.as_deref())
            .or(self.first_prompt.as_deref())
    }

    fn touch_repo(&mut self, resolver: &mut RepoResolver, path: &Path) {
        let Some(root) = resolver.resolve(path) else {
            return;
        };
        let root = root.to_string_lossy().into_owned();
        self.repos.retain(|repo| *repo != root);
        self.repos.insert(0, root);
        self.repos.truncate(MAX_SESSION_REPOS);
    }

    fn apply_line(&mut self, line: &[u8], resolver: &mut RepoResolver) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if self.session_id.is_none() {
            if let Some(id) = non_empty_str(&value, "sessionId") {
                self.session_id = Some(id.to_owned());
            }
        }
        if let Some(cwd) = non_empty_str(&value, "cwd") {
            if self.cwd.as_deref() != Some(cwd) {
                self.cwd = Some(cwd.to_owned());
                self.touch_repo(resolver, Path::new(cwd));
            }
        }
        match value.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                if let Some(title) = non_empty_str(&value, "customTitle") {
                    self.custom_title = Some(truncate_title(title));
                }
            }
            Some("agent-name") => {
                if let Some(name) = non_empty_str(&value, "agentName") {
                    self.agent_name = Some(truncate_title(name));
                }
            }
            Some("user") if self.first_prompt.is_none() => {
                self.first_prompt = first_prompt_text(&value).map(|text| truncate_title(&text));
            }
            Some("assistant") => {
                if let Some(worktree) = last_tool_worktree(&value) {
                    self.tool_worktree = Some(worktree);
                }
                if let Some(path) = last_tool_value(&value, crate::right_panel::worktree_path) {
                    self.tool_worktree_path = Some(path);
                }
                let base = self.cwd.clone().map(PathBuf::from);
                for path in tool_paths(&value, base.as_deref()) {
                    self.touch_repo(resolver, &path);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
pub(crate) struct ClaudeSessionScanner {
    projects_dir: PathBuf,
    transcripts: HashMap<PathBuf, TranscriptState>,
    resolver: RepoResolver,
}

impl ClaudeSessionScanner {
    pub(crate) fn new(claude_dir: &Path) -> Self {
        Self {
            projects_dir: claude_dir.join("projects"),
            transcripts: HashMap::new(),
            resolver: RepoResolver::default(),
        }
    }

    pub(crate) fn scan(&mut self) -> Vec<ClaudeSession> {
        self.resolver.clear();
        let mut candidates = transcript_files(&self.projects_dir);
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.1));
        let mut sessions = Vec::new();
        let mut kept = HashMap::new();
        for (path, modified, len) in candidates {
            if sessions.len() >= MAX_CLAUDE_SESSIONS {
                break;
            }
            let mut state = self.transcripts.remove(&path).unwrap_or_default();
            if state.modified != Some(modified) || state.offset != len {
                if len < state.offset {
                    state = TranscriptState::default();
                }
                if read_transcript(&path, &mut state, &mut self.resolver).is_err() {
                    continue;
                }
                state.modified = Some(modified);
            }
            if let Some(session) = session_from_state(&path, &state, modified) {
                sessions.push(session);
            }
            kept.insert(path, state);
        }
        self.transcripts = kept;
        sessions
    }
}

pub(crate) fn spawn_scanner(
    index: ClaudeSessionIndex,
    render_dirty: Arc<crate::render_signal::RenderSignal>,
    render_notify: Arc<tokio::sync::Notify>,
) {
    let Ok(claude_dir) = crate::integration::claude_dir() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("herdr-claude-sessions".into())
        .spawn(move || {
            let mut scanner = ClaudeSessionScanner::new(&claude_dir);
            loop {
                let sessions = scanner.scan();
                let changed = {
                    let mut current = index
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if *current == sessions {
                        false
                    } else {
                        *current = sessions;
                        true
                    }
                };
                if changed {
                    render_dirty.request_generic();
                    render_notify.notify_one();
                }
                std::thread::sleep(CLAUDE_SESSION_SCAN_INTERVAL);
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(err = %err, "failed to start claude session scanner");
    }
}

fn transcript_files(projects_dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let Ok(projects) = std::fs::read_dir(projects_dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            files.push((path, modified, metadata.len()));
        }
    }
    files
}

fn read_transcript(
    path: &Path,
    state: &mut TranscriptState,
    resolver: &mut RepoResolver,
) -> io::Result<()> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(state.offset))?;
    let mut pending = Vec::new();
    let mut chunk = vec![0; READ_CHUNK_BYTES];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..read]);
        let Some(last_newline) = pending.iter().rposition(|byte| *byte == b'\n') else {
            continue;
        };
        for line in pending[..last_newline].split(|byte| *byte == b'\n') {
            if !line.is_empty() {
                state.apply_line(line, resolver);
            }
        }
        state.offset += (last_newline + 1) as u64;
        pending.drain(..=last_newline);
    }
    Ok(())
}

fn session_from_state(
    path: &Path,
    state: &TranscriptState,
    modified: SystemTime,
) -> Option<ClaudeSession> {
    let session_id = state
        .session_id
        .clone()
        .or_else(|| path.file_stem()?.to_str().map(str::to_owned))
        .filter(|id| crate::agent_resume::AgentSessionRef::id(id).is_some())?;
    let title = state.title()?.to_owned();
    let cwd = state.cwd.clone()?;
    let named_title = state
        .custom_title
        .as_deref()
        .or(state.agent_name.as_deref())
        .unwrap_or_default();
    let context = session_context(&cwd, state.tool_worktree.as_deref(), named_title);
    let worktree_path =
        crate::right_panel::worktree_path(&cwd).or_else(|| state.tool_worktree_path.clone());
    Some(ClaudeSession {
        session_id,
        title,
        cwd,
        context,
        worktree_path,
        repos: state.repos.clone(),
        updated_at_ms: modified
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default(),
    })
}

const WORKTREE_MARKER: &str = "-worktrees/";

fn session_context(cwd: &str, tool_worktree: Option<&str>, title: &str) -> String {
    worktree_name(cwd)
        .or_else(|| tool_worktree.map(str::to_owned))
        .or_else(|| task_code(title))
        .unwrap_or_else(|| {
            Path::new(cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(cwd)
                .to_owned()
        })
}

fn worktree_name(text: &str) -> Option<String> {
    let (_, rest) = text.rsplit_once(WORKTREE_MARKER)?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let name = name.trim_end_matches('.');
    (!name.is_empty()).then(|| name.to_owned())
}

fn last_tool_worktree(value: &Value) -> Option<String> {
    last_tool_value(value, worktree_name)
}

fn last_tool_value(value: &Value, extract: fn(&str) -> Option<String>) -> Option<String> {
    let blocks = value.get("message")?.get("content")?.as_array()?;
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("input"))
        .flat_map(|input| {
            ["command", "file_path", "path", "notebook_path"]
                .into_iter()
                .filter_map(|key| input.get(key).and_then(Value::as_str))
        })
        .filter_map(extract)
        .next_back()
}

#[derive(Debug, Default)]
pub(crate) struct RepoResolver {
    cache: HashMap<PathBuf, Option<PathBuf>>,
}

impl RepoResolver {
    pub(crate) fn clear(&mut self) {
        self.cache.clear();
    }

    pub(crate) fn resolve(&mut self, path: &Path) -> Option<PathBuf> {
        let path = normalize(path)?;
        let start = path.ancestors().find(|ancestor| ancestor.is_dir())?;
        let mut visited = Vec::new();
        let mut found = None;
        for dir in start.ancestors() {
            if let Some(cached) = self.cache.get(dir) {
                found = cached.clone();
                break;
            }
            visited.push(dir.to_path_buf());
            if is_repo_root(dir) {
                found = Some(dir.to_path_buf());
                break;
            }
        }
        if self.cache.len() + visited.len() > MAX_REPO_CACHE {
            self.cache.clear();
        }
        for dir in visited {
            self.cache.insert(dir, found.clone());
        }
        found
    }
}

pub(crate) fn repo_root(path: &Path) -> Option<PathBuf> {
    RepoResolver::default().resolve(path)
}

pub(crate) fn is_repo_root(dir: &Path) -> bool {
    if dir.parent().is_none() {
        return false;
    }
    let dot_git = dir.join(".git");
    if dot_git.is_dir() {
        return dot_git.join("HEAD").is_file();
    }
    std::fs::read_to_string(&dot_git).is_ok_and(|text| text.trim_start().starts_with("gitdir:"))
}

fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    Some(normalized)
}

fn tool_paths(value: &Value, cwd: Option<&Path>) -> Vec<PathBuf> {
    let Some(blocks) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let absolute = |text: &str| -> Option<PathBuf> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let path = expand_tilde(text);
        if path.is_absolute() {
            Some(path)
        } else {
            cwd.map(|cwd| cwd.join(path))
        }
    };
    let mut paths = Vec::new();
    for input in blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("input"))
    {
        for key in ["file_path", "path", "notebook_path"] {
            if let Some(path) = input.get(key).and_then(Value::as_str).and_then(absolute) {
                paths.push(path);
            }
        }
        if let Some(command) = input.get("command").and_then(Value::as_str) {
            paths.extend(command_paths(command, cwd));
        }
    }
    paths
}

fn expand_tilde(text: &str) -> PathBuf {
    match text.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|| PathBuf::from(text)),
        None => PathBuf::from(text),
    }
}

pub(crate) fn command_paths(command: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    let tokens: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`'))
        .map(|token| token.trim_matches(|c| matches!(c, '"' | '\'' | '<' | '>')))
        .filter(|token| !token.is_empty())
        .collect();
    let mut paths = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if paths.len() >= MAX_COMMAND_PATHS {
            break;
        }
        let value = if token.starts_with(['/', '~']) {
            *token
        } else {
            token.split_once('=').map_or(*token, |(_, value)| value)
        };
        let after_dir_flag = index > 0 && matches!(tokens[index - 1], "cd" | "pushd" | "-C");
        if value.starts_with('/') || value.starts_with("~/") {
            paths.push(expand_tilde(value));
        } else if after_dir_flag && !value.starts_with('-') && !value.starts_with('$') {
            if let Some(cwd) = cwd {
                paths.push(cwd.join(value));
            }
        }
    }
    paths
}

fn task_code(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find_map(|word| {
            let (prefix, number) = word.split_once('-')?;
            let valid_prefix = prefix.len() >= 2
                && prefix.starts_with(|c: char| c.is_ascii_uppercase())
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
            let valid_number = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
            (valid_prefix && valid_number).then(|| word.to_owned())
        })
}

fn non_empty_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn first_prompt_text(value: &Value) -> Option<String> {
    if value.get("isMeta").and_then(Value::as_bool) == Some(true)
        || value.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let content = value.get("message")?.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty() && !text.starts_with('<')).then_some(text)
}

fn truncate_title(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= MAX_TITLE_CHARS {
        return text;
    }
    let mut truncated = text.chars().take(MAX_TITLE_CHARS - 1).collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "herdr-claude-sessions-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_transcript(dir: &Path, project: &str, id: &str, lines: &[&str]) -> PathBuf {
        let project_dir = dir.join("projects").join(project);
        std::fs::create_dir_all(&project_dir).unwrap();
        let path = project_dir.join(format!("{id}.jsonl"));
        let mut body = lines.join("\n");
        body.push('\n');
        std::fs::write(&path, body).unwrap();
        path
    }

    fn set_modified(path: &Path, seconds: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn title_prefers_custom_title_then_agent_name_then_first_prompt() {
        let dir = TestDir::new();
        write_transcript(
            dir.path(),
            "p",
            "a",
            &[
                r#"{"type":"user","sessionId":"a","cwd":"/work","message":{"role":"user","content":"first prompt"}}"#,
                r#"{"type":"agent-name","agentName":"agent","sessionId":"a"}"#,
                r#"{"type":"custom-title","customTitle":"old","sessionId":"a"}"#,
                r#"{"type":"custom-title","customTitle":"custom","sessionId":"a"}"#,
            ],
        );
        write_transcript(
            dir.path(),
            "p",
            "b",
            &[
                r#"{"type":"user","sessionId":"b","cwd":"/work","message":{"role":"user","content":"prompt b"}}"#,
                r#"{"type":"agent-name","agentName":"agent b","sessionId":"b"}"#,
            ],
        );
        write_transcript(
            dir.path(),
            "p",
            "c",
            &[
                r#"{"type":"user","isMeta":true,"sessionId":"c","cwd":"/work","message":{"role":"user","content":"meta"}}"#,
                r#"{"type":"user","sessionId":"c","cwd":"/work","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
                r#"{"type":"user","sessionId":"c","cwd":"/work","message":{"role":"user","content":[{"type":"text","text":"real   prompt"}]}}"#,
            ],
        );

        let mut sessions = ClaudeSessionScanner::new(dir.path()).scan();
        sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        let titles = sessions
            .iter()
            .map(|session| session.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["custom", "agent b", "real prompt"]);
        assert!(sessions.iter().all(|session| session.cwd == "/work"));
    }

    #[test]
    fn scan_skips_sessions_without_cwd_or_title_and_tolerates_malformed_lines() {
        let dir = TestDir::new();
        write_transcript(
            dir.path(),
            "p",
            "nocwd",
            &[r#"{"type":"custom-title","customTitle":"x","sessionId":"nocwd"}"#],
        );
        write_transcript(
            dir.path(),
            "p",
            "notitle",
            &[r#"{"type":"mode","sessionId":"notitle","cwd":"/w"}"#],
        );
        write_transcript(
            dir.path(),
            "p",
            "ok",
            &[
                "not json",
                "{\"broken\":",
                r#"{"type":"user","sessionId":"ok","cwd":"/w","message":{"role":"user","content":"hello"}}"#,
            ],
        );

        let sessions = ClaudeSessionScanner::new(dir.path()).scan();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "ok");
        assert_eq!(sessions[0].title, "hello");
    }

    #[test]
    fn scan_orders_by_recency_and_limits_results() {
        let dir = TestDir::new();
        for index in 0..(MAX_CLAUDE_SESSIONS + 5) {
            let id = format!("s{index:02}");
            let line = format!(
                r#"{{"type":"user","sessionId":"{id}","cwd":"/w","message":{{"role":"user","content":"prompt {index}"}}}}"#
            );
            let path = write_transcript(dir.path(), "p", &id, &[&line]);
            set_modified(&path, 1_000 + index as u64);
        }

        let sessions = ClaudeSessionScanner::new(dir.path()).scan();
        assert_eq!(sessions.len(), MAX_CLAUDE_SESSIONS);
        assert_eq!(
            sessions[0].session_id,
            format!("s{:02}", MAX_CLAUDE_SESSIONS + 4)
        );
        assert!(sessions
            .windows(2)
            .all(|pair| pair[0].updated_at_ms >= pair[1].updated_at_ms));
    }

    #[test]
    fn scan_reads_only_appended_lines_and_resets_on_truncation() {
        let dir = TestDir::new();
        let path = write_transcript(
            dir.path(),
            "p",
            "a",
            &[
                r#"{"type":"user","sessionId":"a","cwd":"/w","message":{"role":"user","content":"hello"}}"#,
            ],
        );
        set_modified(&path, 1_000);
        let mut scanner = ClaudeSessionScanner::new(dir.path());
        assert_eq!(scanner.scan()[0].title, "hello");
        let offset = scanner.transcripts[&path].offset;

        let mut file = File::options().append(true).open(&path).unwrap();
        std::io::Write::write_all(
            &mut file,
            b"{\"type\":\"custom-title\",\"customTitle\":\"renamed\",\"sessionId\":\"a\"}\n{\"partial\":",
        )
        .unwrap();
        drop(file);
        set_modified(&path, 1_001);
        assert_eq!(scanner.scan()[0].title, "renamed");
        assert!(scanner.transcripts[&path].offset > offset);
        assert!(scanner.transcripts[&path].offset < std::fs::metadata(&path).unwrap().len());

        std::fs::write(
            &path,
            "{\"type\":\"user\",\"sessionId\":\"a\",\"cwd\":\"/x\",\"message\":{\"role\":\"user\",\"content\":\"new\"}}\n",
        )
        .unwrap();
        set_modified(&path, 1_002);
        let sessions = scanner.scan();
        assert_eq!(sessions[0].title, "new");
        assert_eq!(sessions[0].cwd, "/x");
    }

    #[test]
    fn context_prefers_cwd_worktree_then_tool_worktree_then_title_task_code() {
        let dir = TestDir::new();
        write_transcript(
            dir.path(),
            "p",
            "a",
            &[
                r#"{"type":"user","sessionId":"a","cwd":"/r/vakinha-api-worktrees/VK25-2727-api","message":{"role":"user","content":"hi"}}"#,
            ],
        );
        write_transcript(
            dir.path(),
            "p",
            "b",
            &[
                r#"{"type":"user","sessionId":"b","cwd":"/r","message":{"role":"user","content":"VK25-123 in text only"}}"#,
                r#"{"type":"assistant","sessionId":"b","cwd":"/r","message":{"content":[{"type":"tool_use","input":{"command":"cd /r/web-worktrees/VK25-1 && ls"}}]}}"#,
                r#"{"type":"assistant","sessionId":"b","cwd":"/r","message":{"content":[{"type":"tool_use","input":{"file_path":"/r/web-worktrees/VK25-2904/src/a.ts"}}]}}"#,
            ],
        );
        write_transcript(
            dir.path(),
            "p",
            "c",
            &[
                r#"{"type":"user","sessionId":"c","cwd":"/r/extras","message":{"role":"user","content":"see VK25-9 docs"}}"#,
                r#"{"type":"custom-title","customTitle":"VK25-2806 bug analysis","sessionId":"c"}"#,
            ],
        );
        write_transcript(
            dir.path(),
            "p",
            "d",
            &[
                r#"{"type":"user","sessionId":"d","cwd":"/home/me/projects","message":{"role":"user","content":"mentions VK25-77"}}"#,
            ],
        );

        let mut sessions = ClaudeSessionScanner::new(dir.path()).scan();
        sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        let contexts = sessions
            .iter()
            .map(|session| session.context.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            contexts,
            ["VK25-2727-api", "VK25-2904", "VK25-2806", "projects"]
        );
        let worktree_paths = sessions
            .iter()
            .map(|session| session.worktree_path.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            worktree_paths,
            [
                Some("/r/vakinha-api-worktrees/VK25-2727-api"),
                Some("/r/web-worktrees/VK25-2904"),
                None,
                None
            ]
        );
    }

    fn fake_repo(root: &Path, relative: &str) -> PathBuf {
        let repo = root.join(relative);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        repo
    }

    #[test]
    fn repo_root_accepts_git_dirs_and_worktree_files_but_not_bare_dot_git_folders() {
        let dir = TestDir::new();
        let api = fake_repo(dir.path(), "ws/api");
        std::fs::create_dir_all(api.join("src/deep")).unwrap();
        let worktree = dir.path().join("ws/api-worktrees/VK-1");
        std::fs::create_dir_all(worktree.join("lib")).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: /x/.git/worktrees/VK-1\n").unwrap();
        std::fs::create_dir_all(dir.path().join("ws/.git/info")).unwrap();

        assert_eq!(
            repo_root(&api.join("src/deep/missing.rs")),
            Some(api.clone())
        );
        assert_eq!(repo_root(&api.join("src/../src/deep")), Some(api.clone()));
        assert_eq!(repo_root(&worktree.join("lib/new/file.rb")), Some(worktree));
        assert_eq!(repo_root(&dir.path().join("ws/notes")), None);
        assert_eq!(repo_root(Path::new("relative/path")), None);

        let mut resolver = RepoResolver::default();
        assert_eq!(resolver.resolve(&api.join("src")), Some(api.clone()));
        assert_eq!(
            resolver.cache.get(&api.join("src")),
            Some(&Some(api.clone()))
        );
        assert_eq!(resolver.resolve(&api.join("src/deep")), Some(api));
    }

    #[test]
    fn command_paths_take_absolute_tokens_and_relative_cd_targets() {
        let cwd = Path::new("/w");
        assert_eq!(
            command_paths(
                "cd vakinha-api && git -C \"/w/web-worktrees/VK-2\" status; ls --dir=/opt/x ~/y $HOME/z",
                Some(cwd)
            ),
            [
                PathBuf::from("/w/vakinha-api"),
                PathBuf::from("/w/web-worktrees/VK-2"),
                PathBuf::from("/opt/x"),
                expand_tilde("~/y"),
            ]
        );
        assert!(command_paths("cd -", Some(cwd)).is_empty());
        assert!(command_paths("cd api", None).is_empty());
    }

    #[test]
    fn sessions_accumulate_touched_repos_most_recent_first() {
        let dir = TestDir::new();
        let ws = dir.path().join("ws");
        let api = fake_repo(&ws, "api");
        let web = fake_repo(&ws, "web");
        let engine = fake_repo(&ws, "engine");
        let ws_text = ws.display().to_string();
        let tool = |input: String| {
            format!(
                r#"{{"type":"assistant","sessionId":"a","cwd":"{ws_text}","message":{{"content":[{{"type":"tool_use","input":{input}}}]}}}}"#
            )
        };
        let lines = [
            format!(
                r#"{{"type":"user","sessionId":"a","cwd":"{ws_text}","message":{{"role":"user","content":"hi"}}}}"#
            ),
            tool(format!(r#"{{"file_path":"{}/src/a.rs"}}"#, api.display())),
            tool(r#"{"command":"cd web && git status"}"#.to_owned()),
            tool(format!(r#"{{"path":"{}"}}"#, engine.display())),
            tool(r#"{"command":"cat /definitely/not/a/repo"}"#.to_owned()),
            tool(format!(r#"{{"file_path":"{}/Gemfile"}}"#, api.display())),
            format!(
                r#"{{"type":"user","sessionId":"a","cwd":"{}","message":{{"role":"user","content":"x"}}}}"#,
                web.join("src").display()
            ),
        ];
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        write_transcript(dir.path(), "p", "a", &lines);

        let sessions = ClaudeSessionScanner::new(dir.path()).scan();

        let expected: Vec<String> = [&web, &api, &engine]
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        assert_eq!(sessions[0].repos, expected);
    }

    #[test]
    fn session_repos_are_capped() {
        let dir = TestDir::new();
        let ws = dir.path().join("ws");
        let lines: Vec<String> = (0..MAX_SESSION_REPOS + 3)
            .map(|index| {
                let repo = fake_repo(&ws, &format!("r{index}"));
                format!(
                    r#"{{"type":"assistant","sessionId":"a","cwd":"/","message":{{"content":[{{"type":"tool_use","input":{{"file_path":"{}/x"}}}}]}}}}"#,
                    repo.display()
                )
            })
            .chain([r#"{"type":"user","sessionId":"a","cwd":"/","message":{"role":"user","content":"hi"}}"#.to_owned()])
            .collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        write_transcript(dir.path(), "p", "a", &lines);

        let sessions = ClaudeSessionScanner::new(dir.path()).scan();

        assert_eq!(sessions[0].repos.len(), MAX_SESSION_REPOS);
        assert!(sessions[0].repos[0].ends_with(&format!("r{}", MAX_SESSION_REPOS + 2)));
    }

    #[test]
    fn task_code_requires_uppercase_prefix_and_numeric_suffix() {
        assert_eq!(task_code("fix VK25-12 now").as_deref(), Some("VK25-12"));
        assert_eq!(task_code("ABC-7: thing").as_deref(), Some("ABC-7"));
        assert_eq!(task_code("utf-8 and a-1 and V-2"), None);
        assert_eq!(task_code("VK25-abc"), None);
    }

    #[test]
    fn long_titles_are_truncated() {
        let long = "x".repeat(MAX_TITLE_CHARS + 10);
        let title = truncate_title(&long);
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }
}
