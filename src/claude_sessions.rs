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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeSession {
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    pub context: String,
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
}

impl TranscriptState {
    fn title(&self) -> Option<&str> {
        self.custom_title
            .as_deref()
            .or(self.agent_name.as_deref())
            .or(self.first_prompt.as_deref())
    }

    fn apply_line(&mut self, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if self.session_id.is_none() {
            if let Some(id) = non_empty_str(&value, "sessionId") {
                self.session_id = Some(id.to_owned());
            }
        }
        if let Some(cwd) = non_empty_str(&value, "cwd") {
            self.cwd = Some(cwd.to_owned());
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
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
pub(crate) struct ClaudeSessionScanner {
    projects_dir: PathBuf,
    transcripts: HashMap<PathBuf, TranscriptState>,
}

impl ClaudeSessionScanner {
    pub(crate) fn new(claude_dir: &Path) -> Self {
        Self {
            projects_dir: claude_dir.join("projects"),
            transcripts: HashMap::new(),
        }
    }

    pub(crate) fn scan(&mut self) -> Vec<ClaudeSession> {
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
                if read_transcript(&path, &mut state).is_err() {
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

fn read_transcript(path: &Path, state: &mut TranscriptState) -> io::Result<()> {
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
                state.apply_line(line);
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
    Some(ClaudeSession {
        session_id,
        title,
        cwd,
        context,
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
        .filter_map(worktree_name)
        .next_back()
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
