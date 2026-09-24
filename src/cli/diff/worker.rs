use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use ratatui::text::Line;

use crate::api::schema::{Method, RightPanelOpenParams};
use crate::cli::jira::herdr::{HerdrBridge, SocketBridge};

use super::ansi;
use super::git::{self, FileChange, Mode, RepoDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Session,
    Cwd,
    Nothing,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Job {
    Refresh {
        mode: Mode,
    },
    FileDiff {
        root: PathBuf,
        base_rev: String,
        file: FileChange,
        width: u16,
    },
    Edit {
        root: PathBuf,
        base_rev: String,
        file: FileChange,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Loaded {
        mode: Mode,
        repos: Vec<RepoDiff>,
        source: Source,
    },
    FileDiff {
        root: PathBuf,
        path: String,
        width: u16,
        result: Result<Vec<Line<'static>>, String>,
    },
    Notice(String),
    EditHere {
        path: PathBuf,
        line: u32,
    },
    Failed(String),
}

pub(crate) trait DiffBridge: Send {
    fn session_repos(&self, owner: &str) -> Result<Vec<String>, String>;
    fn open(&self, path: &Path, line: u32, owner: &str) -> Result<(), String>;
}

impl DiffBridge for SocketBridge {
    fn session_repos(&self, owner: &str) -> Result<Vec<String>, String> {
        Ok(self
            .sessions()?
            .into_iter()
            .filter(|session| session.pane_id.as_deref() == Some(owner))
            .max_by_key(|session| session.updated_at_ms)
            .map(|session| session.repos)
            .unwrap_or_default())
    }

    fn open(&self, path: &Path, line: u32, owner: &str) -> Result<(), String> {
        self.call(
            "diff:right_panel:open",
            Method::RightPanelOpen(RightPanelOpenParams {
                path: path.to_string_lossy().into_owned(),
                line: Some(line),
                pane_id: Some(owner.to_owned()),
            }),
        )
        .map(|_| ())
    }
}

pub(crate) struct WorkerContext {
    pub bridge: Box<dyn DiffBridge>,
    pub owner_pane: Option<String>,
    pub cwd: PathBuf,
}

pub(crate) fn run(context: WorkerContext, jobs: Receiver<Job>, outcomes: Sender<Outcome>) {
    let use_delta = git::delta_available();
    while let Ok(job) = jobs.recv() {
        let outcome = handle(&context, use_delta, job);
        if outcomes.send(outcome).is_err() {
            break;
        }
    }
}

fn handle(context: &WorkerContext, use_delta: bool, job: Job) -> Outcome {
    match job {
        Job::Refresh { mode } => {
            let session = context
                .owner_pane
                .as_deref()
                .map(|owner| context.bridge.session_repos(owner).unwrap_or_default())
                .unwrap_or_default();
            let (roots, source) = discover_repos(&session, &context.cwd);
            Outcome::Loaded {
                mode,
                repos: load_all(&roots, mode),
                source,
            }
        }
        Job::FileDiff {
            root,
            base_rev,
            file,
            width,
        } => {
            let result = git::file_diff(&root, &base_rev, &file, width, use_delta)
                .map(|bytes| ansi::parse(&bytes));
            Outcome::FileDiff {
                root,
                path: file.path,
                width,
                result,
            }
        }
        Job::Edit {
            root,
            base_rev,
            file,
        } => {
            let path = root.join(&file.path);
            if !path.is_file() {
                return Outcome::Failed(format!("{} não existe mais", file.path));
            }
            let line = git::first_changed_line(&root, &base_rev, &file);
            match context.owner_pane.as_deref() {
                Some(owner) => match context.bridge.open(&path, line, owner) {
                    Ok(()) => Outcome::Notice(format!("{}:{line} aberto no painel", file.path)),
                    Err(_) => Outcome::EditHere { path, line },
                },
                None => Outcome::EditHere { path, line },
            }
        }
    }
}

pub(crate) fn discover_repos(session: &[String], cwd: &Path) -> (Vec<PathBuf>, Source) {
    let mut roots: Vec<PathBuf> = Vec::new();
    for repo in session {
        let path = PathBuf::from(repo);
        if crate::claude_sessions::is_repo_root(&path) && !roots.contains(&path) {
            roots.push(path);
        }
    }
    let source = if roots.is_empty() {
        Source::Cwd
    } else {
        Source::Session
    };
    if let Some(root) = crate::claude_sessions::repo_root(cwd) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    if roots.is_empty() {
        return (roots, Source::Nothing);
    }
    (roots, source)
}

fn load_all(roots: &[PathBuf], mode: Mode) -> Vec<RepoDiff> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = roots
            .iter()
            .map(|root| scope.spawn(move || git::load_repo(root, mode)))
            .collect();
        handles
            .into_iter()
            .zip(roots)
            .map(|(handle, root)| {
                handle.join().unwrap_or_else(|_| RepoDiff {
                    root: root.clone(),
                    error: Some("falha ao ler o repositório".to_owned()),
                    ..RepoDiff::default()
                })
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_repo(root: &Path, name: &str) -> PathBuf {
        let repo = root.join(name);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        repo
    }

    #[test]
    fn session_repos_come_first_then_the_cwd_repo_and_missing_ones_are_dropped() {
        let root = std::env::temp_dir().join(format!("herdr-diff-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let api = fake_repo(&root, "api");
        let web = fake_repo(&root, "web");
        std::fs::create_dir_all(root.join("plain")).unwrap();
        let session = vec![
            web.display().to_string(),
            root.join("removed").display().to_string(),
            web.display().to_string(),
        ];

        assert_eq!(
            discover_repos(&session, &api.join("src")),
            (vec![web.clone(), api.clone()], Source::Session)
        );
        assert_eq!(discover_repos(&[], &api), (vec![api], Source::Cwd));
        assert_eq!(
            discover_repos(&[], &root.join("plain")),
            (vec![], Source::Nothing)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
