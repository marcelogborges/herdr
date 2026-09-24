use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::api::client::ApiClient;
use crate::api::schema::{
    ClaudeSessionInfo, ClaudeSessionOpenParams, EmptyParams, Method, PaneTarget, Request,
    TabCreateParams,
};

use super::model::{key_matches, Worktree};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkedSession {
    pub session_id: String,
    pub pane_id: Option<String>,
    pub agent_status: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PaneContext {
    pub workspace_id: Option<String>,
    pub cwd: Option<String>,
}

pub(crate) trait HerdrBridge: Send {
    fn sessions(&self) -> Result<Vec<ClaudeSessionInfo>, String>;
    fn pane(&self, pane_id: &str) -> Result<(PaneContext, Option<String>), String>;
    fn focus_pane(&self, pane_id: &str) -> Result<(), String>;
    fn open_session(&self, session_id: &str, workspace_id: Option<String>) -> Result<(), String>;
    fn create_tab(
        &self,
        workspace_id: Option<String>,
        cwd: &Path,
        label: &str,
    ) -> Result<(), String>;
}

pub(crate) struct SocketBridge {
    client: ApiClient,
}

impl SocketBridge {
    pub(crate) fn new() -> Self {
        Self {
            client: ApiClient::local(),
        }
    }

    fn call(&self, id: &str, method: Method) -> Result<Value, String> {
        let value = self
            .client
            .request_value_with_timeout(
                &Request {
                    id: id.to_owned(),
                    method,
                },
                SOCKET_TIMEOUT,
            )
            .map_err(|err| err.to_string())?;
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("erro do herdr");
            return Err(message.to_owned());
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl HerdrBridge for SocketBridge {
    fn sessions(&self) -> Result<Vec<ClaudeSessionInfo>, String> {
        let result = self.call(
            "jira:claude_session:list",
            Method::ClaudeSessionList(EmptyParams::default()),
        )?;
        serde_json::from_value(result.get("sessions").cloned().unwrap_or(Value::Null))
            .map_err(|err| err.to_string())
    }

    fn pane(&self, pane_id: &str) -> Result<(PaneContext, Option<String>), String> {
        let result = self.call(
            "jira:pane:get",
            Method::PaneGet(PaneTarget {
                pane_id: pane_id.to_owned(),
            }),
        )?;
        let pane = result.get("pane").cloned().unwrap_or(Value::Null);
        let text = |key: &str| pane.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok((
            PaneContext {
                workspace_id: text("workspace_id"),
                cwd: text("foreground_cwd").or_else(|| text("cwd")),
            },
            text("agent_status"),
        ))
    }

    fn focus_pane(&self, pane_id: &str) -> Result<(), String> {
        self.call(
            "jira:pane:focus",
            Method::PaneFocus(PaneTarget {
                pane_id: pane_id.to_owned(),
            }),
        )
        .map(|_| ())
    }

    fn open_session(&self, session_id: &str, workspace_id: Option<String>) -> Result<(), String> {
        self.call(
            "jira:claude_session:open",
            Method::ClaudeSessionOpen(ClaudeSessionOpenParams {
                session_id: session_id.to_owned(),
                workspace_id,
            }),
        )
        .map(|_| ())
    }

    fn create_tab(
        &self,
        workspace_id: Option<String>,
        cwd: &Path,
        label: &str,
    ) -> Result<(), String> {
        self.call(
            "jira:tab:create",
            Method::TabCreate(TabCreateParams {
                workspace_id,
                cwd: Some(cwd.to_string_lossy().into_owned()),
                focus: true,
                label: Some(label.to_owned()),
                env: Default::default(),
            }),
        )
        .map(|_| ())
    }
}

pub(crate) fn session_key(session: &ClaudeSessionInfo, keys: &[String]) -> Option<String> {
    let worktree_name = session
        .worktree_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .and_then(|name| name.to_str());
    keys.iter()
        .find(|key| {
            key_matches(&session.context, key)
                || worktree_name.is_some_and(|name| key_matches(name, key))
        })
        .cloned()
}

pub(crate) fn link_sessions(
    sessions: &[ClaudeSessionInfo],
    keys: &[String],
) -> std::collections::HashMap<String, LinkedSession> {
    let mut links: std::collections::HashMap<String, LinkedSession> = Default::default();
    for session in sessions {
        let Some(key) = session_key(session, keys) else {
            continue;
        };
        let candidate = LinkedSession {
            session_id: session.session_id.clone(),
            pane_id: session.pane_id.clone(),
            agent_status: None,
            updated_at_ms: session.updated_at_ms,
        };
        let replace = match links.get(&key) {
            None => true,
            Some(existing) => {
                (candidate.pane_id.is_some(), candidate.updated_at_ms)
                    > (existing.pane_id.is_some(), existing.updated_at_ms)
            }
        };
        if replace {
            links.insert(key, candidate);
        }
    }
    links
}

pub(crate) fn key_for_path(path: &str, keys: &[String]) -> Option<String> {
    let (_, rest) = path.rsplit_once("-worktrees/")?;
    let name = rest.split('/').next()?;
    keys.iter().find(|key| key_matches(name, key)).cloned()
}

pub(crate) fn preselect_key(
    owner_pane: Option<&str>,
    owner_cwd: Option<&str>,
    sessions: &[ClaudeSessionInfo],
    keys: &[String],
) -> Option<String> {
    if let Some(owner) = owner_pane {
        if let Some(key) = sessions
            .iter()
            .filter(|session| session.pane_id.as_deref() == Some(owner))
            .find_map(|session| session_key(session, keys))
        {
            return Some(key);
        }
    }
    owner_cwd.and_then(|cwd| key_for_path(cwd, keys))
}

pub(crate) fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(rest),
        None if path == "~" => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        None => PathBuf::from(path),
    }
}

pub(crate) fn find_worktree(roots: &[String], key: &str) -> Option<PathBuf> {
    find_worktrees(roots, key).into_iter().next()
}

pub(crate) fn find_worktrees(roots: &[String], key: &str) -> Vec<PathBuf> {
    let mut matches = Vec::new();
    for root in roots {
        let root = expand_home(root);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.ends_with("-worktrees") || !entry.path().is_dir() {
                continue;
            }
            let Ok(children) = std::fs::read_dir(entry.path()) else {
                continue;
            };
            for child in children.flatten() {
                let child_name = child.file_name();
                if child_name
                    .to_str()
                    .is_some_and(|child_name| key_matches(child_name, key))
                    && child.path().is_dir()
                {
                    matches.push(child.path());
                }
            }
        }
    }
    matches.sort();
    matches.dedup();
    matches
}

pub(crate) fn worktree_info(path: &Path) -> Worktree {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let repo = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(|name| name.strip_suffix("-worktrees").unwrap_or(name).to_owned())
        .unwrap_or_default();
    Worktree {
        repo,
        name,
        path: path.to_string_lossy().into_owned(),
        branch: git_branch(path),
    }
}

pub(crate) fn git_branch(path: &Path) -> Option<String> {
    let dot_git = path.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir:")?.trim();
        let target = Path::new(target);
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            path.join(target)
        }
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_owned)
}

pub(crate) fn session_in_worktree<'a>(
    sessions: &'a [ClaudeSessionInfo],
    worktree: &Path,
) -> Option<&'a ClaudeSessionInfo> {
    let inside = |path: &str| Path::new(path).starts_with(worktree);
    sessions
        .iter()
        .filter(|session| session.pane_id.is_some())
        .filter(|session| {
            session.worktree_path.as_deref().is_some_and(inside) || inside(&session.cwd)
        })
        .max_by_key(|session| session.updated_at_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, context: &str, pane: Option<&str>, updated: u64) -> ClaudeSessionInfo {
        ClaudeSessionInfo {
            session_id: id.into(),
            title: String::new(),
            cwd: "/home/me/projects".into(),
            context: context.into(),
            worktree_path: None,
            updated_at_ms: updated,
            pane_id: pane.map(str::to_owned),
        }
    }

    fn keys() -> Vec<String> {
        vec!["VK25-2727".into(), "VK25-2904".into()]
    }

    #[test]
    fn sessions_link_by_context_or_worktree_and_prefer_live_recent() {
        let mut by_worktree = session("w", "projects", None, 1);
        by_worktree.worktree_path = Some("/r/vakinha-web-worktrees/VK25-2904".into());
        let sessions = vec![
            session("old", "VK25-2727-api", None, 5),
            session("live", "VK25-2727", Some("w1:p2"), 1),
            session("newer", "VK25-2727", None, 9),
            by_worktree,
            session("other", "VK25-27270", None, 1),
        ];

        let links = link_sessions(&sessions, &keys());

        assert_eq!(links["VK25-2727"].session_id, "live");
        assert_eq!(links["VK25-2904"].session_id, "w");
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn preselect_prefers_owner_session_then_owner_cwd() {
        let sessions = vec![session("s", "VK25-2904", Some("w1:p3"), 1)];

        assert_eq!(
            preselect_key(Some("w1:p3"), None, &sessions, &keys()).as_deref(),
            Some("VK25-2904")
        );
        assert_eq!(
            preselect_key(
                Some("w1:p9"),
                Some("/r/vakinha-api-worktrees/VK25-2727-api/src"),
                &sessions,
                &keys()
            )
            .as_deref(),
            Some("VK25-2727")
        );
        assert_eq!(
            preselect_key(None, Some("/r/projects"), &sessions, &keys()),
            None
        );
    }

    #[test]
    fn worktrees_are_found_under_repo_worktree_dirs() {
        let root = std::env::temp_dir().join(format!("herdr-jira-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("vakinha-web-worktrees/VK25-2904")).unwrap();
        std::fs::create_dir_all(root.join("vakinha-web-worktrees/VK25-29040")).unwrap();
        std::fs::create_dir_all(root.join("plain/VK25-2727")).unwrap();
        let roots = vec![root.to_string_lossy().into_owned()];

        assert_eq!(
            find_worktree(&roots, "VK25-2904"),
            Some(root.join("vakinha-web-worktrees/VK25-2904"))
        );
        assert_eq!(find_worktree(&roots, "VK25-2727"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_worktrees_lists_every_repo_and_reads_branches() {
        let root = std::env::temp_dir().join(format!("herdr-jira-wts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let main_git = root.join("vakinha-api/.git/worktrees/VK25-2907");
        std::fs::create_dir_all(&main_git).unwrap();
        std::fs::write(
            main_git.join("HEAD"),
            "ref: refs/heads/task/VK25-2907/pack\n",
        )
        .unwrap();
        let api = root.join("vakinha-api-worktrees/VK25-2907");
        std::fs::create_dir_all(&api).unwrap();
        std::fs::write(
            api.join(".git"),
            format!("gitdir: {}\n", main_git.display()),
        )
        .unwrap();
        let admin = root.join("vakinha-admin-api-worktrees/VK25-2907");
        std::fs::create_dir_all(admin.join(".git")).unwrap();
        std::fs::write(admin.join(".git/HEAD"), "0123abcd\n").unwrap();
        std::fs::create_dir_all(root.join("vakinha-web-worktrees/VK25-29070")).unwrap();
        let roots = vec![root.to_string_lossy().into_owned()];

        let found = find_worktrees(&roots, "VK25-2907");
        assert_eq!(found, [admin.clone(), api.clone()]);
        let infos: Vec<Worktree> = found.iter().map(|path| worktree_info(path)).collect();
        assert_eq!(infos[0].repo, "vakinha-admin-api");
        assert_eq!(infos[0].name, "VK25-2907");
        assert_eq!(infos[0].branch, None);
        assert_eq!(infos[1].repo, "vakinha-api");
        assert_eq!(infos[1].branch.as_deref(), Some("task/VK25-2907/pack"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn session_in_worktree_prefers_live_recent_sessions_inside_it() {
        let mut inside_cwd = session("a", "x", Some("w1:p1"), 1);
        inside_cwd.cwd = "/r/vakinha-api-worktrees/VK25-1/src".into();
        let mut by_worktree = session("b", "x", Some("w1:p2"), 5);
        by_worktree.worktree_path = Some("/r/vakinha-api-worktrees/VK25-1".into());
        let mut closed = session("c", "x", None, 9);
        closed.worktree_path = Some("/r/vakinha-api-worktrees/VK25-1".into());
        let mut elsewhere = session("d", "x", Some("w1:p3"), 20);
        elsewhere.cwd = "/r/vakinha-api-worktrees/VK25-10".into();
        let sessions = [inside_cwd, by_worktree, closed, elsewhere];

        let found = session_in_worktree(&sessions, Path::new("/r/vakinha-api-worktrees/VK25-1"));
        assert_eq!(found.map(|session| session.session_id.as_str()), Some("b"));
        assert!(session_in_worktree(&sessions, Path::new("/r/other")).is_none());
    }
}
