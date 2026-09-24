use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::git::{FileChange, Mode, RepoDiff};
use super::worker::{Job, Outcome, Source};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const NOTICE_TTL: Duration = Duration::from_secs(6);
const WHEEL_STEP: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    Open,
    Mode,
    Edit,
    Lazygit,
    Refresh,
    Quit,
    Back,
    Next,
    Previous,
}

impl Action {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Open => "enter",
            Self::Mode => "b",
            Self::Edit => "e",
            Self::Lazygit => "g",
            Self::Refresh => "r",
            Self::Quit => "q",
            Self::Back => "esc",
            Self::Next => "n",
            Self::Previous => "p",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Open => "abrir",
            Self::Mode => "modo",
            Self::Edit => "editar",
            Self::Lazygit => "lazygit",
            Self::Refresh => "atualizar",
            Self::Quit => "sair",
            Self::Back => "voltar",
            Self::Next => "próximo",
            Self::Previous => "anterior",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hit {
    Row(usize),
    Button(Action),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Repo(usize),
    File(usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selection {
    Repo(PathBuf),
    File(PathBuf, String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileView {
    pub root: PathBuf,
    pub path: String,
    pub lines: Option<Vec<Line<'static>>>,
    pub error: Option<String>,
    pub scroll: usize,
    pub width: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum View {
    Tree,
    File(FileView),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct External {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub jobs: Vec<Job>,
    pub quit: bool,
    pub external: Option<External>,
}

impl Effects {
    fn job(job: Job) -> Self {
        Self {
            jobs: vec![job],
            ..Self::default()
        }
    }
}

pub(crate) struct DiffState {
    pub mode: Mode,
    pub repos: Vec<RepoDiff>,
    pub source: Option<Source>,
    pub rows: Vec<Row>,
    pub collapsed: HashSet<PathBuf>,
    pub seen: HashSet<PathBuf>,
    pub selection: Option<Selection>,
    pub scroll: usize,
    pub view: View,
    pub notice: Option<Notice>,
    pub loading: bool,
    pub stale: bool,
    pub hits: Vec<(Rect, Hit)>,
    pub list_height: usize,
    pub body_width: u16,
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    last_click: Option<(Instant, Hit)>,
}

impl DiffState {
    pub(crate) fn new(cwd: PathBuf, home: Option<PathBuf>) -> Self {
        Self {
            mode: Mode::default(),
            repos: Vec::new(),
            source: None,
            rows: Vec::new(),
            collapsed: HashSet::new(),
            seen: HashSet::new(),
            selection: None,
            scroll: 0,
            view: View::Tree,
            notice: None,
            loading: false,
            stale: false,
            hits: Vec::new(),
            list_height: 10,
            body_width: 80,
            cwd,
            home,
            last_click: None,
        }
    }

    pub(crate) fn start_refresh(&mut self) -> Option<Job> {
        if self.loading {
            self.stale = true;
            return None;
        }
        self.loading = true;
        self.stale = false;
        Some(Job::Refresh { mode: self.mode })
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Option<Job> {
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| now.duration_since(notice.at) > NOTICE_TTL)
        {
            self.notice = None;
        }
        let width = self.body_width;
        let View::File(view) = &mut self.view else {
            return None;
        };
        if view.lines.is_none() || view.width == width {
            return None;
        }
        view.width = width;
        let (root, path) = (view.root.clone(), view.path.clone());
        self.file_job(&root, &path)
    }

    pub(crate) fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    pub(crate) fn repo_index(&self, root: &Path) -> Option<usize> {
        self.repos.iter().position(|repo| repo.root == root)
    }

    fn find_file(&self, root: &Path, path: &str) -> Option<(usize, usize)> {
        let repo = self.repo_index(root)?;
        let file = self.repos[repo]
            .files
            .iter()
            .position(|file| file.path == path)?;
        Some((repo, file))
    }

    pub(crate) fn file(&self, repo: usize, file: usize) -> Option<&FileChange> {
        self.repos.get(repo)?.files.get(file)
    }

    pub(crate) fn selected_row(&self) -> Option<usize> {
        let selection = self.selection.as_ref()?;
        self.rows
            .iter()
            .position(|row| self.selection_of(*row).as_ref() == Some(selection))
    }

    fn selection_of(&self, row: Row) -> Option<Selection> {
        match row {
            Row::Repo(repo) => Some(Selection::Repo(self.repos.get(repo)?.root.clone())),
            Row::File(repo, file) => {
                let owner = self.repos.get(repo)?;
                Some(Selection::File(
                    owner.root.clone(),
                    owner.files.get(file)?.path.clone(),
                ))
            }
        }
    }

    pub(crate) fn focused_root(&self) -> Option<PathBuf> {
        match &self.view {
            View::File(view) => Some(view.root.clone()),
            View::Tree => match &self.selection {
                Some(Selection::Repo(root) | Selection::File(root, _)) => Some(root.clone()),
                None => None,
            },
        }
    }

    fn focused_file(&self) -> Option<(usize, usize)> {
        match (&self.view, &self.selection) {
            (View::File(view), _) => self.find_file(&view.root, &view.path),
            (View::Tree, Some(Selection::File(root, path))) => self.find_file(root, path),
            _ => None,
        }
    }

    fn rebuild_rows(&mut self) {
        let first_changed = self.repos.iter().position(RepoDiff::has_changes);
        let any_open = self.repos.iter().any(|repo| {
            repo.has_changes()
                && self.seen.contains(&repo.root)
                && !self.collapsed.contains(&repo.root)
        });
        for (index, repo) in self.repos.iter().enumerate() {
            if !repo.has_changes() || !self.seen.insert(repo.root.clone()) {
                continue;
            }
            if any_open || Some(index) != first_changed {
                self.collapsed.insert(repo.root.clone());
            }
        }
        let mut rows = Vec::new();
        for (index, repo) in self.repos.iter().enumerate() {
            rows.push(Row::Repo(index));
            if repo.has_changes() && !self.collapsed.contains(&repo.root) {
                rows.extend((0..repo.files.len()).map(|file| Row::File(index, file)));
            }
        }
        self.rows = rows;
        if self.selected_row().is_none() {
            let parent = match &self.selection {
                Some(Selection::File(root, _)) => {
                    self.repo_index(root).map(|_| Selection::Repo(root.clone()))
                }
                _ => None,
            };
            self.selection = parent
                .or_else(|| {
                    self.rows
                        .iter()
                        .find(|row| matches!(row, Row::File(..)))
                        .and_then(|row| self.selection_of(*row))
                })
                .or_else(|| self.rows.first().and_then(|row| self.selection_of(*row)));
        }
        self.ensure_visible();
    }

    pub(crate) fn is_collapsed(&self, root: &Path) -> bool {
        self.collapsed.contains(root)
    }

    fn select_row(&mut self, index: usize) {
        if let Some(selection) = self.rows.get(index).and_then(|row| self.selection_of(*row)) {
            self.selection = Some(selection);
            self.ensure_visible();
        }
    }

    fn ensure_visible(&mut self) {
        let height = self.list_height.max(1);
        if let Some(row) = self.selected_row() {
            if row < self.scroll {
                self.scroll = row;
            } else if row >= self.scroll + height {
                self.scroll = row + 1 - height;
            }
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(height));
    }

    fn toggle_repo(&mut self, root: &Path) {
        if !self.collapsed.remove(root) {
            self.collapsed.insert(root.to_path_buf());
        }
        self.selection = Some(Selection::Repo(root.to_path_buf()));
        self.rebuild_rows();
    }

    fn file_job(&self, root: &Path, path: &str) -> Option<Job> {
        let (repo, file) = self.find_file(root, path)?;
        let owner = &self.repos[repo];
        Some(Job::FileDiff {
            root: owner.root.clone(),
            base_rev: owner.base_rev.clone(),
            file: owner.files[file].clone(),
            width: self.body_width,
        })
    }

    fn open_file(&mut self, repo: usize, file: usize) -> Effects {
        let Some(Selection::File(root, path)) = self.selection_of(Row::File(repo, file)) else {
            return Effects::default();
        };
        self.selection = Some(Selection::File(root.clone(), path.clone()));
        self.view = View::File(FileView {
            root: root.clone(),
            path: path.clone(),
            lines: None,
            error: None,
            scroll: 0,
            width: self.body_width,
        });
        Effects {
            jobs: self.file_job(&root, &path).into_iter().collect(),
            ..Effects::default()
        }
    }

    fn step_file(&mut self, forward: bool) -> Effects {
        let all: Vec<(usize, usize)> = self
            .repos
            .iter()
            .enumerate()
            .flat_map(|(repo, owner)| (0..owner.files.len()).map(move |file| (repo, file)))
            .collect();
        if all.is_empty() {
            return Effects::default();
        }
        let current = self
            .focused_file()
            .and_then(|focused| all.iter().position(|entry| *entry == focused));
        let next = match (current, forward) {
            (Some(index), true) => (index + 1).min(all.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
            (None, _) => 0,
        };
        if current == Some(next) {
            return Effects::default();
        }
        let (repo, file) = all[next];
        let root = self.repos[repo].root.clone();
        self.collapsed.remove(&root);
        let effects = self.open_file(repo, file);
        self.rebuild_rows();
        effects
    }

    pub(crate) fn action(&mut self, action: Action) -> Effects {
        match action {
            Action::Quit => Effects {
                quit: true,
                ..Effects::default()
            },
            Action::Refresh => Effects {
                jobs: self.start_refresh().into_iter().collect(),
                ..Effects::default()
            },
            Action::Mode => {
                self.mode = self.mode.toggled();
                self.notify(format!("modo {}", self.mode.label()), false);
                Effects {
                    jobs: self.start_refresh().into_iter().collect(),
                    ..Effects::default()
                }
            }
            Action::Back => {
                self.view = View::Tree;
                self.ensure_visible();
                Effects::default()
            }
            Action::Next => self.step_file(true),
            Action::Previous => self.step_file(false),
            Action::Open => match self.selected_row().map(|row| self.rows[row]) {
                Some(Row::Repo(repo)) => {
                    let root = self.repos[repo].root.clone();
                    if self.repos[repo].has_changes() {
                        self.toggle_repo(&root);
                    }
                    Effects::default()
                }
                Some(Row::File(repo, file)) => self.open_file(repo, file),
                None => Effects::default(),
            },
            Action::Edit => {
                let Some((repo, file)) = self.focused_file() else {
                    self.notify("selecione um arquivo", true);
                    return Effects::default();
                };
                let owner = &self.repos[repo];
                let change = owner.files[file].clone();
                if change.status == 'D' {
                    self.notify(format!("{} foi removido", change.path), true);
                    return Effects::default();
                }
                Effects::job(Job::Edit {
                    root: owner.root.clone(),
                    base_rev: owner.base_rev.clone(),
                    file: change,
                })
            }
            Action::Lazygit => match self.focused_root() {
                Some(root) => Effects {
                    external: Some(External {
                        program: "lazygit".to_owned(),
                        args: Vec::new(),
                        dir: root,
                    }),
                    ..Effects::default()
                },
                None => {
                    self.notify("nenhum repositório selecionado", true);
                    Effects::default()
                }
            },
        }
    }

    pub(crate) fn after_external(&mut self, result: Result<(), String>) -> Effects {
        if let Err(error) = result {
            self.notify(error, true);
        }
        Effects {
            jobs: self.start_refresh().into_iter().collect(),
            ..Effects::default()
        }
    }

    pub(crate) fn apply(&mut self, outcome: Outcome) -> Effects {
        let mut effects = Effects::default();
        match outcome {
            Outcome::Loaded {
                mode,
                repos,
                source,
            } => {
                self.loading = false;
                if self.stale || mode != self.mode {
                    effects.jobs.extend(self.start_refresh());
                    return effects;
                }
                let (changed, clean): (Vec<RepoDiff>, Vec<RepoDiff>) =
                    repos.into_iter().partition(RepoDiff::has_changes);
                self.repos = changed.into_iter().chain(clean).collect();
                self.source = Some(source);
                self.rebuild_rows();
                if let View::File(view) = &self.view {
                    let (root, path) = (view.root.clone(), view.path.clone());
                    match self.file_job(&root, &path) {
                        Some(job) => effects.jobs.push(job),
                        None => {
                            self.view = View::Tree;
                            self.notify(format!("{path} não tem mais mudanças"), false);
                        }
                    }
                }
            }
            Outcome::FileDiff {
                root, path, result, ..
            } => {
                if let View::File(view) = &mut self.view {
                    if view.root == root && view.path == path {
                        match result {
                            Ok(lines) => {
                                view.lines = Some(lines);
                                view.error = None;
                            }
                            Err(error) => view.error = Some(error),
                        }
                    }
                }
            }
            Outcome::Notice(text) => self.notify(text, false),
            Outcome::Failed(text) => self.notify(text, true),
            Outcome::EditHere { path, line } => {
                effects.external = Some(External {
                    program: "micro".to_owned(),
                    args: vec![format!("+{line}"), path.to_string_lossy().into_owned()],
                    dir: path
                        .parent()
                        .map_or_else(|| self.cwd.clone(), Path::to_path_buf),
                });
            }
        }
        effects
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Effects {
        let action = match key.code {
            KeyCode::Char('b') => Some(Action::Mode),
            KeyCode::Char('r') => Some(Action::Refresh),
            KeyCode::Char('e') => Some(Action::Edit),
            KeyCode::Char('g') => Some(Action::Lazygit),
            KeyCode::Char('n') => Some(Action::Next),
            KeyCode::Char('p') => Some(Action::Previous),
            _ => None,
        };
        if let Some(action) = action {
            return self.action(action);
        }
        let list_height = self.list_height.max(1);
        if let View::File(view) = &mut self.view {
            match key.code {
                KeyCode::Esc
                | KeyCode::Char('q')
                | KeyCode::Backspace
                | KeyCode::Left
                | KeyCode::Char('h') => return self.action(Action::Back),
                KeyCode::Down | KeyCode::Char('j') => view.scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => view.scroll = view.scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => view.scroll += list_height,
                KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(list_height),
                KeyCode::Home => view.scroll = 0,
                KeyCode::End | KeyCode::Char('G') => view.scroll = usize::MAX,
                _ => {}
            }
            return Effects::default();
        }
        let current = self.selected_row();
        let last = self.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('q') => return self.action(Action::Quit),
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_row(current.map_or(0, |row| (row + 1).min(last)))
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_row(current.map_or(0, |row| row.saturating_sub(1)))
            }
            KeyCode::PageDown => {
                self.select_row(current.map_or(0, |row| (row + list_height).min(last)))
            }
            KeyCode::PageUp => {
                self.select_row(current.map_or(0, |row| row.saturating_sub(list_height)))
            }
            KeyCode::Home => self.select_row(0),
            KeyCode::End | KeyCode::Char('G') => self.select_row(last),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') | KeyCode::Char('l') => {
                return self.action(Action::Open)
            }
            KeyCode::Left | KeyCode::Char('h') => match current.map(|row| self.rows[row]) {
                Some(Row::File(repo, _)) => {
                    let root = self.repos[repo].root.clone();
                    self.toggle_repo(&root);
                }
                Some(Row::Repo(repo)) => {
                    let root = self.repos[repo].root.clone();
                    if self.repos[repo].has_changes() && !self.is_collapsed(&root) {
                        self.toggle_repo(&root);
                    }
                }
                None => {}
            },
            _ => {}
        }
        Effects::default()
    }

    pub(crate) fn mouse(&mut self, mouse: MouseEvent) -> Effects {
        match mouse.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                self.wheel(matches!(mouse.kind, MouseEventKind::ScrollDown));
                Effects::default()
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(rect, _)| contains(*rect, mouse.column, mouse.row))
                    .map(|(_, hit)| hit.clone());
                let now = Instant::now();
                let double = hit.is_some()
                    && self.last_click.as_ref().is_some_and(|(at, previous)| {
                        now.duration_since(*at) <= DOUBLE_CLICK && Some(previous) == hit.as_ref()
                    });
                self.last_click = hit.clone().map(|hit| (now, hit));
                match hit {
                    Some(Hit::Button(action)) => self.action(action),
                    Some(Hit::Row(index)) => self.click_row(index, double),
                    None => Effects::default(),
                }
            }
            _ => Effects::default(),
        }
    }

    fn click_row(&mut self, index: usize, double: bool) -> Effects {
        match self.rows.get(index).copied() {
            Some(Row::Repo(repo)) => {
                let root = self.repos[repo].root.clone();
                if self.repos[repo].has_changes() {
                    self.toggle_repo(&root);
                } else {
                    self.select_row(index);
                }
                Effects::default()
            }
            Some(Row::File(repo, file)) => {
                self.select_row(index);
                if double {
                    self.last_click = None;
                    return self.open_file(repo, file);
                }
                Effects::default()
            }
            None => Effects::default(),
        }
    }

    fn wheel(&mut self, down: bool) {
        match &mut self.view {
            View::File(view) => {
                view.scroll = if down {
                    view.scroll.saturating_add(WHEEL_STEP)
                } else {
                    view.scroll.saturating_sub(WHEEL_STEP)
                };
            }
            View::Tree => {
                let max = self.rows.len().saturating_sub(self.list_height.max(1));
                self.scroll = if down {
                    (self.scroll + WHEEL_STEP).min(max)
                } else {
                    self.scroll.saturating_sub(WHEEL_STEP)
                };
            }
        }
    }
}

pub(crate) fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

    pub(crate) fn change(path: &str, status: char, adds: u64, dels: u64) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: None,
            status,
            adds,
            dels,
            binary: false,
        }
    }

    pub(crate) fn repo(root: &str, files: Vec<FileChange>) -> RepoDiff {
        RepoDiff {
            root: root.into(),
            branch: Some("task/x".into()),
            base_label: "origin/develop".into(),
            base_rev: "abc".into(),
            files,
            error: None,
        }
    }

    pub(crate) fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn loaded(state: &mut DiffState, repos: Vec<RepoDiff>) -> Effects {
        let job = state.start_refresh();
        assert_eq!(job, Some(Job::Refresh { mode: state.mode }));
        state.apply(Outcome::Loaded {
            mode: state.mode,
            repos,
            source: Source::Session,
        })
    }

    pub(crate) fn loaded_state() -> DiffState {
        let mut state = DiffState::new("/w".into(), Some("/home/me".into()));
        loaded(
            &mut state,
            vec![
                repo("/w/clean", vec![]),
                repo(
                    "/w/api",
                    vec![
                        change("app/a.rb", 'M', 3, 1),
                        change("spec/a_spec.rb", 'A', 9, 0),
                    ],
                ),
                repo("/w/web", vec![change("src/b.ts", 'D', 0, 4)]),
            ],
        );
        state
    }

    #[test]
    fn first_changed_repo_opens_and_others_collapse_with_clean_repos_last() {
        let state = loaded_state();

        let roots: Vec<&Path> = state.repos.iter().map(|repo| repo.root.as_path()).collect();
        assert_eq!(
            roots,
            [
                Path::new("/w/api"),
                Path::new("/w/web"),
                Path::new("/w/clean")
            ]
        );
        assert_eq!(
            state.rows,
            [
                Row::Repo(0),
                Row::File(0, 0),
                Row::File(0, 1),
                Row::Repo(1),
                Row::Repo(2)
            ]
        );
        assert_eq!(
            state.selection,
            Some(Selection::File("/w/api".into(), "app/a.rb".into()))
        );
    }

    #[test]
    fn refresh_keeps_manual_expansion_and_new_repos_arrive_collapsed() {
        let mut state = loaded_state();
        state.selection = Some(Selection::Repo("/w/web".into()));
        state.key(press(KeyCode::Enter));
        assert!(!state.is_collapsed(Path::new("/w/web")));

        loaded(
            &mut state,
            vec![
                repo("/w/new", vec![change("x", '?', 1, 0)]),
                repo("/w/api", vec![change("app/a.rb", 'M', 3, 1)]),
                repo("/w/web", vec![change("src/b.ts", 'D', 0, 4)]),
            ],
        );

        assert!(state.is_collapsed(Path::new("/w/new")));
        assert!(!state.is_collapsed(Path::new("/w/api")));
        assert!(!state.is_collapsed(Path::new("/w/web")));
    }

    #[test]
    fn mode_toggle_refreshes_and_discards_results_of_the_old_mode() {
        let mut state = loaded_state();
        assert_eq!(state.mode, Mode::Pr);

        let effects = state.key(press(KeyCode::Char('b')));
        assert_eq!(
            effects.jobs,
            [Job::Refresh {
                mode: Mode::Uncommitted
            }]
        );
        assert!(state.key(press(KeyCode::Char('b'))).jobs.is_empty());
        assert_eq!(state.mode, Mode::Pr);

        let effects = state.apply(Outcome::Loaded {
            mode: Mode::Uncommitted,
            repos: vec![],
            source: Source::Cwd,
        });
        assert_eq!(effects.jobs, [Job::Refresh { mode: Mode::Pr }]);
        assert_eq!(state.repos.len(), 3);
    }

    #[test]
    fn enter_opens_file_diff_and_escape_returns_to_the_tree() {
        let mut state = loaded_state();
        state.body_width = 70;

        let effects = state.key(press(KeyCode::Enter));

        let [Job::FileDiff {
            root, file, width, ..
        }] = effects.jobs.as_slice()
        else {
            panic!("{:?}", effects.jobs);
        };
        assert_eq!(root, Path::new("/w/api"));
        assert_eq!(file.path, "app/a.rb");
        assert_eq!(*width, 70);
        state.apply(Outcome::FileDiff {
            root: "/w/api".into(),
            path: "app/a.rb".into(),
            width: 70,
            result: Ok(vec![Line::from("diff")]),
        });
        let View::File(view) = &state.view else {
            panic!("expected file view");
        };
        assert_eq!(view.lines.as_ref().map(Vec::len), Some(1));
        state.key(press(KeyCode::Char('j')));
        state.key(press(KeyCode::Esc));
        assert_eq!(state.view, View::Tree);
    }

    #[test]
    fn next_file_crosses_repos_and_expands_the_target() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Down));
        let effects = state.key(press(KeyCode::Char('n')));

        let [Job::FileDiff { root, file, .. }] = effects.jobs.as_slice() else {
            panic!("{:?}", effects.jobs);
        };
        assert_eq!(
            (root.as_path(), file.path.as_str()),
            (Path::new("/w/web"), "src/b.ts")
        );
        assert!(!state.is_collapsed(Path::new("/w/web")));
        assert!(state.key(press(KeyCode::Char('n'))).jobs.is_empty());
    }

    #[test]
    fn edit_needs_an_existing_file_and_lazygit_runs_in_the_selected_repo() {
        let mut state = loaded_state();
        let effects = state.key(press(KeyCode::Char('e')));
        assert!(
            matches!(effects.jobs.as_slice(), [Job::Edit { file, .. }] if file.path == "app/a.rb")
        );

        state.selection = Some(Selection::File("/w/web".into(), "src/b.ts".into()));
        assert!(state.key(press(KeyCode::Char('e'))).jobs.is_empty());
        assert!(state.notice.as_ref().is_some_and(|notice| notice.error));

        let effects = state.key(press(KeyCode::Char('g')));
        assert_eq!(
            effects.external,
            Some(External {
                program: "lazygit".into(),
                args: vec![],
                dir: "/w/web".into(),
            })
        );
        let effects = state.after_external(Ok(()));
        assert_eq!(effects.jobs, [Job::Refresh { mode: Mode::Pr }]);
    }

    #[test]
    fn edit_fallback_runs_micro_at_the_line() {
        let mut state = loaded_state();
        let effects = state.apply(Outcome::EditHere {
            path: "/w/api/app/a.rb".into(),
            line: 12,
        });
        assert_eq!(
            effects.external,
            Some(External {
                program: "micro".into(),
                args: vec!["+12".into(), "/w/api/app/a.rb".into()],
                dir: "/w/api/app".into(),
            })
        );
    }

    #[test]
    fn refresh_in_file_view_reloads_the_file_or_falls_back_to_the_tree() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Enter));

        let effects = loaded(
            &mut state,
            vec![repo("/w/api", vec![change("app/a.rb", 'M', 5, 1)])],
        );
        assert!(matches!(effects.jobs.as_slice(), [Job::FileDiff { file, .. }] if file.adds == 5));

        loaded(&mut state, vec![repo("/w/api", vec![])]);
        assert_eq!(state.view, View::Tree);
        assert_eq!(state.selection, Some(Selection::Repo("/w/api".into())));
    }

    #[test]
    fn width_change_reloads_an_open_diff_once() {
        let mut state = loaded_state();
        state.body_width = 60;
        state.key(press(KeyCode::Enter));
        let now = Instant::now();
        assert_eq!(state.tick(now), None);
        state.apply(Outcome::FileDiff {
            root: "/w/api".into(),
            path: "app/a.rb".into(),
            width: 60,
            result: Ok(vec![]),
        });
        state.body_width = 90;
        assert!(matches!(
            state.tick(now),
            Some(Job::FileDiff { width: 90, .. })
        ));
        assert_eq!(state.tick(now), None);
    }
}
