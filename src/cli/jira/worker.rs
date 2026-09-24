use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use crate::api::schema::ClaudeSessionInfo;

use super::api::JiraApi;
use super::gc::{self, GcReport};
use super::herdr::{
    find_worktree, find_worktrees, session_in_worktree, worktree_info, HerdrBridge, LinkedSession,
    PaneContext,
};
use super::model::{Comment, Issue, IssueDetail, Transition};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Job {
    Refresh,
    Detail(String),
    Transitions(String),
    Transition {
        key: String,
        transition_id: String,
        to: String,
    },
    Comment {
        key: String,
        text: String,
    },
    AssignMe(String),
    Session {
        key: String,
        link: Option<LinkedSession>,
    },
    OpenUrl(String),
    OpenWorktree {
        path: String,
        name: String,
    },
    WorktreeGc {
        apply: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    List {
        issues: Vec<Issue>,
        sessions: Vec<ClaudeSessionInfo>,
        owner: PaneContext,
        pane_status: HashMap<String, String>,
    },
    ListFailed(String),
    Detail(Box<IssueDetail>),
    DetailFailed {
        key: String,
        error: String,
    },
    Transitions {
        key: String,
        transitions: Vec<Transition>,
    },
    TransitionsFailed {
        key: String,
        error: String,
    },
    Transitioned {
        key: String,
        to: String,
    },
    Commented {
        key: String,
        comment: Comment,
    },
    Assigned {
        key: String,
        name: String,
    },
    CommentFailed(String),
    WorktreeGc {
        apply: bool,
        result: Result<GcReport, String>,
    },
    Notice(String),
    Failed(String),
}

pub(crate) struct WorkerContext {
    pub api: JiraApi,
    pub bridge: Box<dyn HerdrBridge>,
    pub jql: String,
    pub owner_pane: Option<String>,
    pub worktree_roots: Vec<String>,
    pub browser_command: String,
    pub worktree_gc_command: String,
}

pub(crate) fn run(context: WorkerContext, jobs: Receiver<Job>, outcomes: Sender<Outcome>) {
    let mut worker = Worker {
        context,
        myself: None,
    };
    while let Ok(job) = jobs.recv() {
        if let Job::WorktreeGc { apply } = job {
            let command = worker.context.worktree_gc_command.clone();
            let gc_outcomes = outcomes.clone();
            let spawned = std::thread::Builder::new()
                .name("herdr-jira-gc".into())
                .spawn(move || {
                    let result = gc::run(&command, apply);
                    let _ = gc_outcomes.send(Outcome::WorktreeGc { apply, result });
                });
            if let Err(err) = spawned {
                let outcome = Outcome::WorktreeGc {
                    apply,
                    result: Err(format!("não foi possível iniciar o limpador: {err}")),
                };
                if outcomes.send(outcome).is_err() {
                    break;
                }
            }
            continue;
        }
        let outcome = worker.handle(job);
        if outcomes.send(outcome).is_err() {
            break;
        }
    }
}

struct Worker {
    context: WorkerContext,
    myself: Option<(String, String)>,
}

impl Worker {
    fn handle(&mut self, job: Job) -> Outcome {
        let api = &self.context.api;
        match job {
            Job::Refresh => match api.search(&self.context.jql) {
                Ok(issues) => {
                    let sessions = self.context.bridge.sessions().unwrap_or_default();
                    let owner = self
                        .context
                        .owner_pane
                        .as_deref()
                        .and_then(|pane| self.context.bridge.pane(pane).ok())
                        .map(|(context, _)| context)
                        .unwrap_or_default();
                    let pane_status = sessions
                        .iter()
                        .filter_map(|session| session.pane_id.as_deref())
                        .filter_map(|pane| {
                            let (_, status) = self.context.bridge.pane(pane).ok()?;
                            Some((pane.to_owned(), status?))
                        })
                        .collect();
                    Outcome::List {
                        issues,
                        sessions,
                        owner,
                        pane_status,
                    }
                }
                Err(err) => Outcome::ListFailed(err.to_string()),
            },
            Job::Detail(key) => match api.issue_detail(&key) {
                Ok(mut detail) => {
                    detail.worktrees = find_worktrees(&self.context.worktree_roots, &key)
                        .iter()
                        .map(|path| worktree_info(path))
                        .collect();
                    Outcome::Detail(Box::new(detail))
                }
                Err(err) => Outcome::DetailFailed {
                    key,
                    error: err.to_string(),
                },
            },
            Job::Transitions(key) => match api.transitions(&key) {
                Ok(transitions) => Outcome::Transitions { key, transitions },
                Err(err) => Outcome::TransitionsFailed {
                    key,
                    error: err.to_string(),
                },
            },
            Job::Transition {
                key,
                transition_id,
                to,
            } => match api.apply_transition(&key, &transition_id) {
                Ok(()) => Outcome::Transitioned { key, to },
                Err(err) => Outcome::Failed(format!("não foi possível mover {key}: {err}")),
            },
            Job::Comment { key, text } => match api.add_comment(&key, &text) {
                Ok(comment) => Outcome::Commented { key, comment },
                Err(err) => Outcome::CommentFailed(format!("comentário não enviado: {err}")),
            },
            Job::AssignMe(key) => {
                if self.myself.is_none() {
                    match api.myself() {
                        Ok(myself) => self.myself = Some(myself),
                        Err(err) => {
                            return Outcome::Failed(format!("não foi possível atribuir: {err}"))
                        }
                    }
                }
                let (account_id, name) = self.myself.clone().unwrap_or_default();
                match api.assign(&key, &account_id) {
                    Ok(()) => Outcome::Assigned { key, name },
                    Err(err) => Outcome::Failed(format!("não foi possível atribuir {key}: {err}")),
                }
            }
            Job::Session { key, link } => self.open_session(&key, link),
            Job::OpenWorktree { path, name } => self.open_worktree(&path, &name),
            Job::WorktreeGc { apply } => Outcome::WorktreeGc {
                apply,
                result: gc::run(&self.context.worktree_gc_command, apply),
            },
            Job::OpenUrl(url) => match open_url(&self.context.browser_command, &url) {
                Ok(()) => Outcome::Notice(format!("abrindo {url}")),
                Err(err) => Outcome::Failed(format!("não foi possível abrir o browser: {err}")),
            },
        }
    }

    fn owner_workspace(&self) -> Option<String> {
        self.context
            .owner_pane
            .as_deref()
            .and_then(|pane| self.context.bridge.pane(pane).ok())
            .and_then(|(context, _)| context.workspace_id)
    }

    fn open_worktree(&self, path: &str, name: &str) -> Outcome {
        let bridge = &self.context.bridge;
        let sessions = bridge.sessions().unwrap_or_default();
        if let Some(pane) = session_in_worktree(&sessions, std::path::Path::new(path))
            .and_then(|session| session.pane_id.as_deref())
        {
            return match bridge.focus_pane(pane) {
                Ok(()) => Outcome::Notice(format!("{name}: sessão focada")),
                Err(err) => Outcome::Failed(format!("não foi possível focar a sessão: {err}")),
            };
        }
        match bridge.create_tab(self.owner_workspace(), std::path::Path::new(path), name) {
            Ok(()) => Outcome::Notice(format!("{name}: tab aberta em {path}")),
            Err(err) => Outcome::Failed(format!("não foi possível abrir a tab: {err}")),
        }
    }

    fn open_session(&self, key: &str, link: Option<LinkedSession>) -> Outcome {
        let bridge = &self.context.bridge;
        let workspace = self
            .context
            .owner_pane
            .as_deref()
            .and_then(|pane| bridge.pane(pane).ok())
            .and_then(|(context, _)| context.workspace_id);
        if let Some(link) = link {
            if let Some(pane) = link.pane_id.as_deref() {
                return match bridge.focus_pane(pane) {
                    Ok(()) => Outcome::Notice(format!("{key}: sessão focada")),
                    Err(err) => Outcome::Failed(format!("não foi possível focar a sessão: {err}")),
                };
            }
            return match bridge.open_session(&link.session_id, workspace) {
                Ok(()) => Outcome::Notice(format!("{key}: sessão retomada em nova tab")),
                Err(err) => Outcome::Failed(format!("não foi possível abrir a sessão: {err}")),
            };
        }
        match find_worktree(&self.context.worktree_roots, key) {
            Some(path) => match bridge.create_tab(workspace, &path, key) {
                Ok(()) => Outcome::Notice(format!("{key}: tab aberta em {}", path.display())),
                Err(err) => Outcome::Failed(format!("não foi possível abrir a tab: {err}")),
            },
            None => Outcome::Notice(format!("sem sessão/worktree para {key}")),
        }
    }
}

pub(crate) fn open_url(template: &str, url: &str) -> std::io::Result<()> {
    let quoted = format!("'{}'", url.replace('\'', "'\\''"));
    let command = if template.contains("{url}") {
        template.replace("{url}", &quoted)
    } else {
        format!("{template} {quoted}")
    };
    crate::noninteractive_process::command("/bin/sh")
        .args(["-c", &command])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cli::jira::api::tests::{api_for, mock_jira};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub(crate) struct FakeBridge {
        pub sessions: Vec<ClaudeSessionInfo>,
        pub calls: Arc<Mutex<Vec<String>>>,
    }

    impl HerdrBridge for FakeBridge {
        fn sessions(&self) -> Result<Vec<ClaudeSessionInfo>, String> {
            Ok(self.sessions.clone())
        }

        fn pane(&self, pane_id: &str) -> Result<(PaneContext, Option<String>), String> {
            Ok((
                PaneContext {
                    workspace_id: Some("w1".into()),
                    cwd: Some(format!("/cwd/{pane_id}")),
                },
                Some("working".into()),
            ))
        }

        fn focus_pane(&self, pane_id: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("focus {pane_id}"));
            Ok(())
        }

        fn open_session(
            &self,
            session_id: &str,
            workspace_id: Option<String>,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!(
                "open {session_id} {}",
                workspace_id.unwrap_or_default()
            ));
            Ok(())
        }

        fn create_tab(
            &self,
            workspace_id: Option<String>,
            cwd: &Path,
            label: &str,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!(
                "tab {} {} {label}",
                workspace_id.unwrap_or_default(),
                cwd.display()
            ));
            Ok(())
        }
    }

    fn worker_with(
        bridge: FakeBridge,
        site_routes: Vec<(&'static str, &'static str, u16, String)>,
    ) -> Worker {
        let mock = mock_jira(site_routes);
        Worker {
            context: WorkerContext {
                api: api_for(&mock),
                bridge: Box::new(bridge),
                jql: "project = X".into(),
                owner_pane: Some("w1:p1".into()),
                worktree_roots: Vec::new(),
                browser_command: "true {url}".into(),
                worktree_gc_command: "true".into(),
            },
            myself: None,
        }
    }

    #[test]
    fn session_job_focuses_live_pane_or_resumes_closed_session() {
        let bridge = FakeBridge::default();
        let calls = bridge.calls.clone();
        let worker = worker_with(bridge, Vec::new());
        let live = LinkedSession {
            session_id: "s1".into(),
            pane_id: Some("w1:p4".into()),
            agent_status: None,
            updated_at_ms: 0,
        };
        let closed = LinkedSession {
            pane_id: None,
            ..live.clone()
        };

        worker.open_session("VK25-1", Some(live));
        worker.open_session("VK25-1", Some(closed));
        let none = worker.open_session("VK25-1", None);

        assert_eq!(*calls.lock().unwrap(), ["focus w1:p4", "open s1 w1"]);
        assert_eq!(
            none,
            Outcome::Notice("sem sessão/worktree para VK25-1".into())
        );
    }

    #[test]
    fn assign_me_fetches_myself_once_then_assigns() {
        let mut worker = worker_with(
            FakeBridge::default(),
            vec![
                (
                    "GET",
                    "/rest/api/3/myself",
                    200,
                    r#"{"accountId":"acc-9","displayName":"Marcelo"}"#.into(),
                ),
                ("PUT", "/rest/api/3/issue/", 204, String::new()),
            ],
        );

        let first = worker.handle(Job::AssignMe("VK25-1".into()));
        let second = worker.handle(Job::AssignMe("VK25-2".into()));

        assert_eq!(
            first,
            Outcome::Assigned {
                key: "VK25-1".into(),
                name: "Marcelo".into()
            }
        );
        assert_eq!(
            second,
            Outcome::Assigned {
                key: "VK25-2".into(),
                name: "Marcelo".into()
            }
        );
    }

    #[test]
    fn refresh_collects_sessions_owner_and_live_pane_status() {
        let bridge = FakeBridge {
            sessions: vec![ClaudeSessionInfo {
                session_id: "s".into(),
                title: String::new(),
                cwd: String::new(),
                context: "VK25-1".into(),
                worktree_path: None,
                updated_at_ms: 0,
                pane_id: Some("w1:p7".into()),
            }],
            ..FakeBridge::default()
        };
        let mut worker = worker_with(
            bridge,
            vec![(
                "GET",
                "/rest/api/3/search/jql",
                200,
                r#"{"issues":[]}"#.into(),
            )],
        );

        let Outcome::List {
            owner,
            pane_status,
            sessions,
            ..
        } = worker.handle(Job::Refresh)
        else {
            panic!("expected list");
        };

        assert_eq!(owner.workspace_id.as_deref(), Some("w1"));
        assert_eq!(pane_status["w1:p7"], "working");
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn open_worktree_focuses_a_live_session_or_opens_a_tab() {
        let mut live = ClaudeSessionInfo {
            session_id: "s1".into(),
            title: String::new(),
            cwd: "/r/vakinha-api-worktrees/VK25-1".into(),
            context: String::new(),
            worktree_path: None,
            updated_at_ms: 1,
            pane_id: Some("w1:p7".into()),
        };
        let bridge = FakeBridge {
            sessions: vec![live.clone()],
            ..FakeBridge::default()
        };
        let calls = bridge.calls.clone();
        let mut worker = worker_with(bridge, Vec::new());
        worker.handle(Job::OpenWorktree {
            path: "/r/vakinha-api-worktrees/VK25-1".into(),
            name: "VK25-1".into(),
        });
        assert_eq!(calls.lock().unwrap().as_slice(), ["focus w1:p7"]);

        live.pane_id = None;
        let bridge = FakeBridge {
            sessions: vec![live],
            ..FakeBridge::default()
        };
        let calls = bridge.calls.clone();
        let mut worker = worker_with(bridge, Vec::new());
        worker.handle(Job::OpenWorktree {
            path: "/r/vakinha-api-worktrees/VK25-1".into(),
            name: "VK25-1".into(),
        });
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["tab w1 /r/vakinha-api-worktrees/VK25-1 VK25-1"]
        );
    }
}
