use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::gc::GcReport;
use super::herdr::{link_sessions, preselect_key, LinkedSession};
use super::model::{group_issues, Issue, IssueDetail, Transition};
use super::worker::{Job, Outcome};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const NOTICE_TTL: Duration = Duration::from_secs(6);
const WHEEL_STEP: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    Move,
    Comment,
    AssignMe,
    Browser,
    Session,
    Refresh,
    Back,
    Send,
    Cancel,
    CleanWorktrees,
    Confirm,
    Close,
}

impl Action {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Move => "m mover",
            Self::Comment => "c comentar",
            Self::AssignMe => "a atribuir a mim",
            Self::Browser => "o browser",
            Self::Session => "w sessão",
            Self::Refresh => "r atualizar",
            Self::Back => "esc voltar",
            Self::Send => "ctrl+s enviar",
            Self::Cancel => "esc cancelar",
            Self::CleanWorktrees => "L limpar worktrees",
            Self::Confirm => "enter confirmar",
            Self::Close => "esc fechar",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DetailSection {
    PullRequests,
    Worktrees,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hit {
    Row(usize),
    Section(DetailSection),
    Worktree(usize),
    Dot(String),
    Button(Action),
    MenuItem(usize),
    Link(String),
    PopupArea,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    Header {
        status: String,
        count: usize,
        collapsed: bool,
    },
    Issue(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selection {
    Header(String),
    Issue(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum View {
    List,
    Detail {
        key: String,
        detail: Option<Box<IssueDetail>>,
        error: Option<String>,
        scroll: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Popup {
    Transitions {
        key: String,
        transitions: Option<Vec<Transition>>,
        error: Option<String>,
        selected: usize,
    },
    Comment {
        key: String,
        text: String,
        cursor: usize,
        sending: bool,
    },
    WorktreeGc {
        phase: GcPhase,
        scroll: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GcPhase {
    Loading,
    Preview(GcReport),
    Applying(GcReport),
    Done(GcReport),
    Failed(String),
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
}

pub(crate) struct JiraState {
    pub project: String,
    pub browse_base: String,
    pub status_order: Vec<String>,
    pub refresh_every: Option<Duration>,
    pub owner_pane: Option<String>,
    pub issues: Vec<Issue>,
    pub rows: Vec<Row>,
    pub collapsed: HashSet<String>,
    pub seen_groups: HashSet<String>,
    pub collapsed_sections: HashSet<DetailSection>,
    pub selection: Option<Selection>,
    pub scroll: usize,
    pub links: HashMap<String, LinkedSession>,
    pub view: View,
    pub popup: Option<Popup>,
    pub notice: Option<Notice>,
    pub loading: bool,
    pub list_error: Option<String>,
    pub last_refresh: Option<Instant>,
    pub preselected: bool,
    pub hits: Vec<(Rect, Hit)>,
    pub list_height: usize,
    pub detail_lines: usize,
    pub detail_height: usize,
    last_click: Option<(Instant, Hit)>,
}

impl JiraState {
    pub(crate) fn new(
        project: String,
        browse_base: String,
        status_order: Vec<String>,
        refresh_seconds: u64,
        owner_pane: Option<String>,
    ) -> Self {
        Self {
            project,
            browse_base,
            status_order,
            refresh_every: (refresh_seconds > 0).then(|| Duration::from_secs(refresh_seconds)),
            owner_pane,
            issues: Vec::new(),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            seen_groups: HashSet::new(),
            collapsed_sections: HashSet::from([
                DetailSection::PullRequests,
                DetailSection::Worktrees,
            ]),
            selection: None,
            scroll: 0,
            links: HashMap::new(),
            view: View::List,
            popup: None,
            notice: None,
            loading: false,
            list_error: None,
            last_refresh: None,
            preselected: false,
            hits: Vec::new(),
            list_height: 10,
            detail_lines: 0,
            detail_height: 10,
            last_click: None,
        }
    }

    pub(crate) fn start_refresh(&mut self) -> Option<Job> {
        if self.loading {
            return None;
        }
        self.loading = true;
        Some(Job::Refresh)
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Option<Job> {
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| now.duration_since(notice.at) > NOTICE_TTL)
        {
            self.notice = None;
        }
        let every = self.refresh_every?;
        let due = self
            .last_refresh
            .is_none_or(|last| now.duration_since(last) >= every);
        if due && self.popup.is_none() {
            self.start_refresh()
        } else {
            None
        }
    }

    pub(crate) fn issue(&self, key: &str) -> Option<&Issue> {
        self.issues.iter().find(|issue| issue.key == key)
    }

    pub(crate) fn selected_row(&self) -> Option<usize> {
        let selection = self.selection.as_ref()?;
        self.rows.iter().position(|row| match (row, selection) {
            (Row::Header { status, .. }, Selection::Header(selected)) => status == selected,
            (Row::Issue(key), Selection::Issue(selected)) => key == selected,
            _ => false,
        })
    }

    pub(crate) fn target_key(&self) -> Option<String> {
        match &self.view {
            View::Detail { key, .. } => Some(key.clone()),
            View::List => match &self.selection {
                Some(Selection::Issue(key)) => Some(key.clone()),
                _ => None,
            },
        }
    }

    fn rebuild_rows(&mut self) {
        let groups = group_issues(&self.issues, &self.status_order);
        let highlighted = match &self.selection {
            Some(Selection::Issue(key)) => self.issue(key).map(|issue| issue.status.clone()),
            _ => None,
        };
        let mut rows = Vec::new();
        for group in groups {
            if self.seen_groups.insert(group.status.clone())
                && highlighted.as_deref() != Some(group.status.as_str())
            {
                self.collapsed.insert(group.status.clone());
            }
            let collapsed = self.collapsed.contains(&group.status);
            rows.push(Row::Header {
                status: group.status.clone(),
                count: group.issues.len(),
                collapsed,
            });
            if !collapsed {
                rows.extend(group.issues.into_iter().map(|issue| Row::Issue(issue.key)));
            }
        }
        self.rows = rows;
        if self.selected_row().is_none() {
            let fallback = match &self.selection {
                Some(Selection::Issue(key)) => self
                    .issue(key)
                    .map(|issue| Selection::Header(issue.status.clone())),
                _ => None,
            };
            self.selection = fallback
                .filter(|selection| {
                    let Selection::Header(status) = selection else {
                        return false;
                    };
                    self.rows
                        .iter()
                        .any(|row| matches!(row, Row::Header { status: s, .. } if s == status))
                })
                .or_else(|| {
                    self.rows.iter().find_map(|row| match row {
                        Row::Issue(key) => Some(Selection::Issue(key.clone())),
                        _ => None,
                    })
                })
                .or_else(|| {
                    self.rows.first().map(|row| match row {
                        Row::Header { status, .. } => Selection::Header(status.clone()),
                        Row::Issue(key) => Selection::Issue(key.clone()),
                    })
                });
        }
        self.ensure_visible();
    }

    fn select_row(&mut self, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        self.selection = Some(match row {
            Row::Header { status, .. } => Selection::Header(status.clone()),
            Row::Issue(key) => Selection::Issue(key.clone()),
        });
        self.ensure_visible();
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
        let max = self.rows.len().saturating_sub(height);
        self.scroll = self.scroll.min(max);
    }

    pub(crate) fn toggle_section(&mut self, section: DetailSection) {
        if !self.collapsed_sections.remove(&section) {
            self.collapsed_sections.insert(section);
        }
    }

    fn open_worktree(&self, index: usize) -> Option<Job> {
        let View::Detail {
            detail: Some(detail),
            ..
        } = &self.view
        else {
            return None;
        };
        let worktree = detail.worktrees.get(index)?;
        Some(Job::OpenWorktree {
            path: worktree.path.clone(),
            name: worktree.name.clone(),
        })
    }

    fn toggle_group(&mut self, status: &str) {
        if !self.collapsed.remove(status) {
            self.collapsed.insert(status.to_owned());
        }
        self.selection = Some(Selection::Header(status.to_owned()));
        self.rebuild_rows();
    }

    fn open_detail(&mut self, key: String) -> Job {
        self.selection = Some(Selection::Issue(key.clone()));
        self.view = View::Detail {
            key: key.clone(),
            detail: None,
            error: None,
            scroll: 0,
        };
        Job::Detail(key)
    }

    pub(crate) fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    pub(crate) fn apply(&mut self, outcome: Outcome) -> Effects {
        let mut effects = Effects::default();
        match outcome {
            Outcome::List {
                issues,
                sessions,
                owner,
                pane_status,
            } => {
                self.loading = false;
                self.list_error = None;
                self.last_refresh = Some(Instant::now());
                let keys: Vec<String> = issues.iter().map(|issue| issue.key.clone()).collect();
                let mut links = link_sessions(&sessions, &keys);
                for link in links.values_mut() {
                    link.agent_status = link
                        .pane_id
                        .as_ref()
                        .and_then(|pane| pane_status.get(pane))
                        .cloned();
                }
                self.links = links;
                self.issues = issues;
                if !self.preselected {
                    self.preselected = true;
                    if let Some(key) = preselect_key(
                        self.owner_pane.as_deref(),
                        owner.cwd.as_deref(),
                        &sessions,
                        &keys,
                    ) {
                        self.selection = Some(Selection::Issue(key));
                    }
                }
                self.rebuild_rows();
            }
            Outcome::ListFailed(error) => {
                self.loading = false;
                self.last_refresh = Some(Instant::now());
                self.list_error = Some(error);
            }
            Outcome::Detail(detail) => {
                if let View::Detail {
                    key,
                    detail: slot,
                    error,
                    ..
                } = &mut self.view
                {
                    if *key == detail.issue.key {
                        *slot = Some(detail);
                        *error = None;
                    }
                }
            }
            Outcome::DetailFailed { key, error } => {
                if let View::Detail {
                    key: current,
                    error: slot,
                    ..
                } = &mut self.view
                {
                    if *current == key {
                        *slot = Some(error);
                    }
                }
            }
            Outcome::Transitions { key, transitions } => {
                if let Some(Popup::Transitions {
                    key: current,
                    transitions: slot,
                    ..
                }) = &mut self.popup
                {
                    if *current == key {
                        *slot = Some(transitions);
                    }
                }
            }
            Outcome::TransitionsFailed { key, error } => {
                if let Some(Popup::Transitions {
                    key: current,
                    error: slot,
                    ..
                }) = &mut self.popup
                {
                    if *current == key {
                        *slot = Some(error);
                    }
                }
            }
            Outcome::Transitioned { key, to } => {
                self.notify(format!("{key} → {to}"), false);
                effects.jobs.extend(self.start_refresh());
                if matches!(&self.view, View::Detail { key: current, .. } if *current == key) {
                    effects.jobs.push(Job::Detail(key));
                }
            }
            Outcome::Commented { key, comment } => {
                if matches!(&self.popup, Some(Popup::Comment { key: current, .. }) if *current == key)
                {
                    self.popup = None;
                }
                if let View::Detail {
                    key: current,
                    detail: Some(detail),
                    ..
                } = &mut self.view
                {
                    if *current == key {
                        detail.comments.push(comment);
                    }
                }
                self.notify(format!("comentário enviado em {key}"), false);
            }
            Outcome::CommentFailed(error) => {
                if let Some(Popup::Comment { sending, .. }) = &mut self.popup {
                    *sending = false;
                }
                self.notify(error, true);
            }
            Outcome::Assigned { key, name } => {
                if let Some(issue) = self.issues.iter_mut().find(|issue| issue.key == key) {
                    issue.assignee = Some(name.clone());
                }
                if let View::Detail {
                    key: current,
                    detail: Some(detail),
                    ..
                } = &mut self.view
                {
                    if *current == key {
                        detail.issue.assignee = Some(name.clone());
                    }
                }
                self.notify(format!("{key} atribuída a {name}"), false);
                effects.jobs.extend(self.start_refresh());
            }
            Outcome::WorktreeGc { apply, result } => {
                let phase = match (&self.popup, apply) {
                    (
                        Some(Popup::WorktreeGc {
                            phase: GcPhase::Loading,
                            ..
                        }),
                        false,
                    ) => Some(match &result {
                        Ok(report) => GcPhase::Preview(report.clone()),
                        Err(error) => GcPhase::Failed(error.clone()),
                    }),
                    (
                        Some(Popup::WorktreeGc {
                            phase: GcPhase::Applying(_),
                            ..
                        }),
                        true,
                    ) => Some(match &result {
                        Ok(report) => GcPhase::Done(report.clone()),
                        Err(error) => GcPhase::Failed(error.clone()),
                    }),
                    _ => None,
                };
                match phase {
                    Some(phase) => self.popup = Some(Popup::WorktreeGc { phase, scroll: 0 }),
                    None if apply => match &result {
                        Ok(report) => self.notify(
                            report
                                .summary
                                .clone()
                                .unwrap_or_else(|| "limpeza de worktrees concluída".into()),
                            false,
                        ),
                        Err(error) => self.notify(error.clone(), true),
                    },
                    None => {}
                }
                if apply {
                    effects.jobs.extend(self.start_refresh());
                    if let View::Detail { key, .. } = &self.view {
                        effects.jobs.push(Job::Detail(key.clone()));
                    }
                }
            }
            Outcome::Notice(text) => self.notify(text, false),
            Outcome::Failed(text) => self.notify(text, true),
        }
        effects
    }

    pub(crate) fn action(&mut self, action: Action) -> Effects {
        let mut effects = Effects::default();
        match action {
            Action::Refresh => {
                effects.jobs.extend(self.start_refresh());
                if let View::Detail { key, .. } = &self.view {
                    effects.jobs.push(Job::Detail(key.clone()));
                }
            }
            Action::Back => {
                if self.popup.take().is_none() {
                    self.view = View::List;
                    self.ensure_visible();
                }
            }
            Action::Cancel | Action::Close => self.popup = None,
            Action::CleanWorktrees => {
                if !matches!(
                    self.popup,
                    Some(Popup::WorktreeGc {
                        phase: GcPhase::Applying(_),
                        ..
                    })
                ) {
                    self.popup = Some(Popup::WorktreeGc {
                        phase: GcPhase::Loading,
                        scroll: 0,
                    });
                    effects.jobs.push(Job::WorktreeGc { apply: false });
                }
            }
            Action::Confirm => {
                if let Some(Popup::WorktreeGc { phase, scroll }) = &mut self.popup {
                    match phase {
                        GcPhase::Preview(report) if report.has_work() => {
                            *phase = GcPhase::Applying(report.clone());
                            *scroll = 0;
                            effects.jobs.push(Job::WorktreeGc { apply: true });
                        }
                        GcPhase::Preview(_) | GcPhase::Done(_) | GcPhase::Failed(_) => {
                            self.popup = None
                        }
                        GcPhase::Loading | GcPhase::Applying(_) => {}
                    }
                }
            }
            Action::Send => effects.jobs.extend(self.send_comment()),
            Action::Move
            | Action::Comment
            | Action::AssignMe
            | Action::Browser
            | Action::Session => {
                let Some(key) = self.target_key() else {
                    self.notify("selecione uma task", true);
                    return effects;
                };
                match action {
                    Action::Move => {
                        self.popup = Some(Popup::Transitions {
                            key: key.clone(),
                            transitions: None,
                            error: None,
                            selected: 0,
                        });
                        effects.jobs.push(Job::Transitions(key));
                    }
                    Action::Comment => {
                        self.popup = Some(Popup::Comment {
                            key,
                            text: String::new(),
                            cursor: 0,
                            sending: false,
                        });
                    }
                    Action::AssignMe => effects.jobs.push(Job::AssignMe(key)),
                    Action::Browser => effects
                        .jobs
                        .push(Job::OpenUrl(format!("{}{key}", self.browse_base))),
                    Action::Session => {
                        let link = self.links.get(&key).cloned();
                        effects.jobs.push(Job::Session { key, link });
                    }
                    _ => {}
                }
            }
        }
        effects
    }

    fn send_comment(&mut self) -> Option<Job> {
        let Some(Popup::Comment {
            key, text, sending, ..
        }) = &mut self.popup
        else {
            return None;
        };
        if *sending {
            return None;
        }
        if text.trim().is_empty() {
            self.notify("comentário vazio", true);
            return None;
        }
        *sending = true;
        Some(Job::Comment {
            key: key.clone(),
            text: text.clone(),
        })
    }

    fn apply_transition(&mut self, index: usize) -> Effects {
        let mut effects = Effects::default();
        let Some(Popup::Transitions {
            key,
            transitions: Some(transitions),
            ..
        }) = &self.popup
        else {
            return effects;
        };
        let Some(transition) = transitions.get(index).cloned() else {
            return effects;
        };
        let key = key.clone();
        self.popup = None;
        if !transition.required_fields.is_empty() {
            self.notify(
                format!(
                    "\"{}\" pede {}; abrindo no browser",
                    transition.name,
                    transition.required_fields.join(", ")
                ),
                true,
            );
            effects
                .jobs
                .push(Job::OpenUrl(format!("{}{key}", self.browse_base)));
            return effects;
        }
        if let Some(issue) = self.issues.iter_mut().find(|issue| issue.key == key) {
            issue.status = transition.to.clone();
        }
        if let View::Detail {
            key: current,
            detail: Some(detail),
            ..
        } = &mut self.view
        {
            if *current == key {
                detail.issue.status = transition.to.clone();
            }
        }
        self.rebuild_rows();
        self.notify(format!("movendo {key} → {}…", transition.to), false);
        effects.jobs.push(Job::Transition {
            key,
            transition_id: transition.id,
            to: transition.to,
        });
        effects
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Effects {
        if let Some(popup) = &mut self.popup {
            match popup {
                Popup::Comment {
                    text,
                    cursor,
                    sending,
                    ..
                } => {
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    match key.code {
                        KeyCode::Esc => self.popup = None,
                        KeyCode::Char('s') if ctrl => return self.action(Action::Send),
                        KeyCode::Enter if ctrl || key.modifiers.contains(KeyModifiers::ALT) => {
                            return self.action(Action::Send)
                        }
                        _ if *sending => {}
                        KeyCode::Enter => insert(text, cursor, "\n"),
                        KeyCode::Char(ch) if !ctrl => insert(text, cursor, &ch.to_string()),
                        KeyCode::Tab => insert(text, cursor, "  "),
                        KeyCode::Backspace => {
                            if let Some(previous) = text[..*cursor].chars().next_back() {
                                *cursor -= previous.len_utf8();
                                text.remove(*cursor);
                            }
                        }
                        KeyCode::Delete => {
                            if *cursor < text.len() {
                                text.remove(*cursor);
                            }
                        }
                        KeyCode::Left => {
                            if let Some(previous) = text[..*cursor].chars().next_back() {
                                *cursor -= previous.len_utf8();
                            }
                        }
                        KeyCode::Right => {
                            if let Some(next) = text[*cursor..].chars().next() {
                                *cursor += next.len_utf8();
                            }
                        }
                        KeyCode::Home => *cursor = text[..*cursor].rfind('\n').map_or(0, |i| i + 1),
                        KeyCode::End => {
                            *cursor = text[*cursor..]
                                .find('\n')
                                .map_or(text.len(), |i| *cursor + i)
                        }
                        _ => {}
                    }
                    return Effects::default();
                }
                Popup::WorktreeGc { scroll, .. } => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => return self.action(Action::Close),
                        KeyCode::Enter => return self.action(Action::Confirm),
                        KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                        KeyCode::PageUp => *scroll = scroll.saturating_sub(5),
                        KeyCode::PageDown => *scroll += 5,
                        _ => {}
                    }
                    return Effects::default();
                }
                Popup::Transitions {
                    transitions,
                    selected,
                    ..
                } => {
                    let count = transitions.as_ref().map_or(0, Vec::len);
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                        KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('j') => {
                            *selected = (*selected + 1).min(count.saturating_sub(1))
                        }
                        KeyCode::Enter => {
                            let index = *selected;
                            return self.apply_transition(index);
                        }
                        KeyCode::Char(ch) if ch.is_ascii_digit() && ch != '0' => {
                            let index = ch as usize - '1' as usize;
                            if index < count {
                                return self.apply_transition(index);
                            }
                        }
                        _ => {}
                    }
                    return Effects::default();
                }
            }
        }
        let action = match key.code {
            KeyCode::Char('m') => Some(Action::Move),
            KeyCode::Char('c') => Some(Action::Comment),
            KeyCode::Char('a') => Some(Action::AssignMe),
            KeyCode::Char('o') => Some(Action::Browser),
            KeyCode::Char('w') => Some(Action::Session),
            KeyCode::Char('r') => Some(Action::Refresh),
            KeyCode::Char('L') => Some(Action::CleanWorktrees),
            _ => None,
        };
        if let Some(action) = action {
            return self.action(action);
        }
        let mut effects = Effects::default();
        match &mut self.view {
            View::Detail { scroll, .. } => match key.code {
                KeyCode::Char('p') => self.toggle_section(DetailSection::PullRequests),
                KeyCode::Char('t') => self.toggle_section(DetailSection::Worktrees),
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') | KeyCode::Left => {
                    return self.action(Action::Back)
                }
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll += self.detail_height.max(1),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(self.detail_height.max(1)),
                KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                KeyCode::End | KeyCode::Char('G') => *scroll = usize::MAX,
                _ => {}
            },
            View::List => {
                let current = self.selected_row();
                let last = self.rows.len().saturating_sub(1);
                match key.code {
                    KeyCode::Char('q') => effects.quit = true,
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.select_row(current.map_or(0, |row| (row + 1).min(last)))
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.select_row(current.map_or(0, |row| row.saturating_sub(1)))
                    }
                    KeyCode::PageDown => self.select_row(
                        current.map_or(0, |row| (row + self.list_height.max(1)).min(last)),
                    ),
                    KeyCode::PageUp => self.select_row(
                        current.map_or(0, |row| row.saturating_sub(self.list_height.max(1))),
                    ),
                    KeyCode::Home | KeyCode::Char('g') => self.select_row(0),
                    KeyCode::End | KeyCode::Char('G') => self.select_row(last),
                    KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => {
                        match self.selection.clone() {
                            Some(Selection::Header(status)) => self.toggle_group(&status),
                            Some(Selection::Issue(key)) => effects.jobs.push(self.open_detail(key)),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        effects
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        if let Some(Popup::Comment {
            text,
            cursor,
            sending: false,
            ..
        }) = &mut self.popup
        {
            insert(
                text,
                cursor,
                &pasted.replace("\r\n", "\n").replace('\r', "\n"),
            );
        }
    }

    pub(crate) fn mouse(&mut self, mouse: MouseEvent) -> Effects {
        match mouse.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = matches!(mouse.kind, MouseEventKind::ScrollDown);
                self.wheel(down);
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
                    Some(hit) => self.click(hit, double),
                    None => {
                        if matches!(self.popup, Some(Popup::Transitions { .. })) {
                            self.popup = None;
                        }
                        Effects::default()
                    }
                }
            }
            _ => Effects::default(),
        }
    }

    fn wheel(&mut self, down: bool) {
        if let Some(Popup::Transitions {
            transitions,
            selected,
            ..
        }) = &mut self.popup
        {
            let count = transitions.as_ref().map_or(0, Vec::len);
            *selected = if down {
                (*selected + 1).min(count.saturating_sub(1))
            } else {
                selected.saturating_sub(1)
            };
            return;
        }
        if let Some(Popup::WorktreeGc { scroll, .. }) = &mut self.popup {
            *scroll = if down {
                *scroll + WHEEL_STEP
            } else {
                scroll.saturating_sub(WHEEL_STEP)
            };
            return;
        }
        if self.popup.is_some() {
            return;
        }
        match &mut self.view {
            View::Detail { scroll, .. } => {
                *scroll = if down {
                    *scroll + WHEEL_STEP
                } else {
                    scroll.saturating_sub(WHEEL_STEP)
                };
            }
            View::List => {
                let max = self.rows.len().saturating_sub(self.list_height.max(1));
                self.scroll = if down {
                    (self.scroll + WHEEL_STEP).min(max)
                } else {
                    self.scroll.saturating_sub(WHEEL_STEP)
                };
            }
        }
    }

    fn click(&mut self, hit: Hit, double: bool) -> Effects {
        let mut effects = Effects::default();
        match hit {
            Hit::Button(action) => return self.action(action),
            Hit::MenuItem(index) => {
                if let Some(Popup::Transitions { selected, .. }) = &mut self.popup {
                    *selected = index;
                }
                return self.apply_transition(index);
            }
            Hit::PopupArea => {}
            Hit::Section(section) => self.toggle_section(section),
            Hit::Worktree(index) => effects.jobs.extend(self.open_worktree(index)),
            Hit::Link(url) => effects.jobs.push(Job::OpenUrl(url)),
            Hit::Dot(key) => {
                self.selection = Some(Selection::Issue(key));
                return self.action(Action::Session);
            }
            Hit::Row(index) => {
                let Some(row) = self.rows.get(index).cloned() else {
                    return effects;
                };
                match row {
                    Row::Header { status, .. } => self.toggle_group(&status),
                    Row::Issue(key) => {
                        self.select_row(index);
                        if double {
                            self.last_click = None;
                            effects.jobs.push(self.open_detail(key));
                        }
                    }
                }
            }
        }
        effects
    }
}

fn insert(text: &mut String, cursor: &mut usize, value: &str) {
    text.insert_str(*cursor, value);
    *cursor += value.len();
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
    use crate::cli::jira::model::StatusCategory;
    use crossterm::event::KeyEventState;

    pub(crate) fn issue(key: &str, status: &str) -> Issue {
        Issue {
            key: key.into(),
            summary: format!("resumo {key}"),
            status: status.into(),
            category: StatusCategory::InProgress,
            priority: Some("High".into()),
            assignee: None,
            issue_type: "Task".into(),
            updated: String::new(),
            parent: None,
        }
    }

    pub(crate) fn loaded_state() -> JiraState {
        let mut state = freshly_loaded_state();
        state.collapsed.clear();
        state.rebuild_rows();
        state
    }

    fn freshly_loaded_state() -> JiraState {
        let mut state = JiraState::new(
            "VK25".into(),
            "https://x.atlassian.net/browse/".into(),
            vec!["Development".into(), "Code Review".into()],
            60,
            Some("w1:p1".into()),
        );
        state.loading = true;
        state.apply(Outcome::List {
            issues: vec![
                issue("VK25-1", "Code Review"),
                issue("VK25-2", "Development"),
                issue("VK25-3", "Code Review"),
            ],
            sessions: vec![crate::api::schema::ClaudeSessionInfo {
                session_id: "s3".into(),
                title: String::new(),
                cwd: String::new(),
                context: "VK25-3".into(),
                worktree_path: None,
                repos: Vec::new(),
                updated_at_ms: 1,
                pane_id: Some("w1:p1".into()),
            }],
            owner: Default::default(),
            pane_status: [("w1:p1".to_owned(), "working".to_owned())].into(),
        });
        state
    }

    pub(crate) fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..press(KeyCode::Char(ch))
        }
    }

    #[test]
    fn list_groups_rows_and_preselects_owner_session_issue() {
        let state = loaded_state();

        assert_eq!(
            state.rows,
            [
                Row::Header {
                    status: "Development".into(),
                    count: 1,
                    collapsed: false
                },
                Row::Issue("VK25-2".into()),
                Row::Header {
                    status: "Code Review".into(),
                    count: 2,
                    collapsed: false
                },
                Row::Issue("VK25-1".into()),
                Row::Issue("VK25-3".into()),
            ]
        );
        assert_eq!(state.selection, Some(Selection::Issue("VK25-3".into())));
        assert_eq!(
            state.links["VK25-3"].agent_status.as_deref(),
            Some("working")
        );
        assert!(!state.loading);
    }

    #[test]
    fn first_load_collapses_every_group_except_the_highlighted_issue() {
        let mut state = freshly_loaded_state();

        assert_eq!(
            state.rows,
            [
                Row::Header {
                    status: "Development".into(),
                    count: 1,
                    collapsed: true
                },
                Row::Header {
                    status: "Code Review".into(),
                    count: 2,
                    collapsed: false
                },
                Row::Issue("VK25-1".into()),
                Row::Issue("VK25-3".into()),
            ]
        );
        assert_eq!(state.selection, Some(Selection::Issue("VK25-3".into())));

        state.toggle_group("Development");
        state.issues.push(issue("VK25-4", "QA - Staging"));
        state.rebuild_rows();
        assert!(state.rows.contains(&Row::Issue("VK25-2".into())));
        assert!(state.rows.contains(&Row::Header {
            status: "QA - Staging".into(),
            count: 1,
            collapsed: true
        }));
    }

    #[test]
    fn enter_on_header_collapses_and_on_issue_opens_detail() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Home));
        state.key(press(KeyCode::Enter));

        assert!(state.rows.contains(&Row::Header {
            status: "Development".into(),
            count: 1,
            collapsed: true
        }));
        assert!(!state.rows.contains(&Row::Issue("VK25-2".into())));

        state.key(press(KeyCode::Down));
        state.key(press(KeyCode::Down));
        let effects = state.key(press(KeyCode::Enter));

        assert_eq!(effects.jobs, [Job::Detail("VK25-1".into())]);
        assert!(matches!(&state.view, View::Detail { key, .. } if key == "VK25-1"));
        state.key(press(KeyCode::Esc));
        assert_eq!(state.view, View::List);
    }

    #[test]
    fn move_menu_applies_transition_optimistically() {
        let mut state = loaded_state();
        let effects = state.key(press(KeyCode::Char('m')));
        assert_eq!(effects.jobs, [Job::Transitions("VK25-3".into())]);
        state.apply(Outcome::Transitions {
            key: "VK25-3".into(),
            transitions: vec![
                Transition {
                    id: "1".into(),
                    name: "Voltar".into(),
                    to: "Development".into(),
                    required_fields: vec![],
                },
                Transition {
                    id: "2".into(),
                    name: "Merge".into(),
                    to: "QA - Staging".into(),
                    required_fields: vec![],
                },
            ],
        });

        state.key(press(KeyCode::Down));
        let effects = state.key(press(KeyCode::Enter));

        assert_eq!(
            effects.jobs,
            [Job::Transition {
                key: "VK25-3".into(),
                transition_id: "2".into(),
                to: "QA - Staging".into()
            }]
        );
        assert_eq!(state.issue("VK25-3").unwrap().status, "QA - Staging");
        assert!(state.popup.is_none());
        assert!(state.rows.contains(&Row::Header {
            status: "QA - Staging".into(),
            count: 1,
            collapsed: false
        }));
    }

    #[test]
    fn transitions_with_required_fields_open_the_browser_instead() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Char('m')));
        state.apply(Outcome::Transitions {
            key: "VK25-3".into(),
            transitions: vec![Transition {
                id: "9".into(),
                name: "Resolver".into(),
                to: "Concluído".into(),
                required_fields: vec!["Resolução".into()],
            }],
        });

        let effects = state.key(press(KeyCode::Char('1')));

        assert_eq!(
            effects.jobs,
            [Job::OpenUrl("https://x.atlassian.net/browse/VK25-3".into())]
        );
        assert_eq!(state.issue("VK25-3").unwrap().status, "Code Review");
        assert!(state.notice.as_ref().unwrap().error);
    }

    #[test]
    fn comment_popup_edits_and_sends_with_ctrl_s() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Char('c')));
        for ch in "oi".chars() {
            state.key(press(KeyCode::Char(ch)));
        }
        state.key(press(KeyCode::Enter));
        state.paste("tchau\r\n");
        state.key(press(KeyCode::Backspace));

        let effects = state.key(ctrl('s'));

        assert_eq!(
            effects.jobs,
            [Job::Comment {
                key: "VK25-3".into(),
                text: "oi\ntchau".into()
            }]
        );
        assert!(state.key(ctrl('s')).jobs.is_empty());
        state.apply(Outcome::Commented {
            key: "VK25-3".into(),
            comment: crate::cli::jira::model::Comment {
                author: "Eu".into(),
                created: String::new(),
                body: serde_json::Value::Null,
            },
        });
        assert!(state.popup.is_none());
    }

    #[test]
    fn empty_comment_is_not_sent_and_escape_cancels() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Char('c')));

        assert!(state.key(ctrl('s')).jobs.is_empty());
        state.key(press(KeyCode::Esc));
        assert!(state.popup.is_none());
    }

    #[test]
    fn actions_need_a_selected_issue() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Home));

        assert!(state.key(press(KeyCode::Char('a'))).jobs.is_empty());
        assert!(state.notice.as_ref().unwrap().error);
        state.key(press(KeyCode::Down));
        assert_eq!(
            state.key(press(KeyCode::Char('a'))).jobs,
            [Job::AssignMe("VK25-2".into())]
        );
        assert_eq!(
            state.key(press(KeyCode::Char('o'))).jobs,
            [Job::OpenUrl("https://x.atlassian.net/browse/VK25-2".into())]
        );
    }

    #[test]
    fn session_action_carries_the_linked_session() {
        let mut state = loaded_state();

        let effects = state.key(press(KeyCode::Char('w')));

        let [Job::Session {
            key,
            link: Some(link),
        }] = effects.jobs.as_slice()
        else {
            panic!("expected session job, got {:?}", effects.jobs);
        };
        assert_eq!(key, "VK25-3");
        assert_eq!(link.pane_id.as_deref(), Some("w1:p1"));
    }

    #[test]
    fn auto_refresh_waits_for_interval_and_skips_while_loading() {
        let mut state = loaded_state();
        let now = Instant::now();

        assert_eq!(state.tick(now), None);
        assert_eq!(
            state.tick(now + Duration::from_secs(61)),
            Some(Job::Refresh)
        );
        assert_eq!(state.tick(now + Duration::from_secs(122)), None);
    }

    #[test]
    fn list_failures_keep_existing_rows() {
        let mut state = loaded_state();
        state.loading = true;

        state.apply(Outcome::ListFailed("falha de rede".into()));

        assert_eq!(state.rows.len(), 5);
        assert_eq!(state.list_error.as_deref(), Some("falha de rede"));
    }

    fn gc_report(removable: &[&str]) -> GcReport {
        GcReport {
            summary: Some(format!(
                "3 worktrees | {} removiveis (~1MB) | 0 orfas | 2 mantidas",
                removable.len()
            )),
            removable: removable.iter().map(|row| (*row).to_owned()).collect(),
            orphans: Vec::new(),
            lines: Vec::new(),
        }
    }

    fn shift(ch: char) -> KeyEvent {
        KeyEvent {
            modifiers: KeyModifiers::SHIFT,
            ..press(KeyCode::Char(ch))
        }
    }

    #[test]
    fn clean_worktrees_runs_dry_run_then_applies_only_after_confirm() {
        let mut state = loaded_state();
        let effects = state.key(shift('L'));
        assert_eq!(effects.jobs, [Job::WorktreeGc { apply: false }]);
        assert_eq!(
            state.popup,
            Some(Popup::WorktreeGc {
                phase: GcPhase::Loading,
                scroll: 0
            })
        );
        assert!(state.key(press(KeyCode::Enter)).jobs.is_empty());

        state.apply(Outcome::WorktreeGc {
            apply: false,
            result: Ok(gc_report(&["a-worktrees/K-1  pr=merged  1MB"])),
        });
        assert!(matches!(
            state.popup,
            Some(Popup::WorktreeGc {
                phase: GcPhase::Preview(_),
                ..
            })
        ));

        let effects = state.key(press(KeyCode::Enter));
        assert_eq!(effects.jobs, [Job::WorktreeGc { apply: true }]);
        assert!(matches!(
            state.popup,
            Some(Popup::WorktreeGc {
                phase: GcPhase::Applying(_),
                ..
            })
        ));
        assert!(state.action(Action::Confirm).jobs.is_empty());

        state.loading = false;
        let effects = state.apply(Outcome::WorktreeGc {
            apply: true,
            result: Ok(GcReport {
                summary: Some("1 worktrees removidas, 0 orfas podadas.".into()),
                ..GcReport::default()
            }),
        });
        assert!(effects.jobs.contains(&Job::Refresh));
        assert!(matches!(
            state.popup,
            Some(Popup::WorktreeGc {
                phase: GcPhase::Done(_),
                ..
            })
        ));
        state.key(press(KeyCode::Esc));
        assert_eq!(state.popup, None);
    }

    #[test]
    fn nothing_to_remove_only_closes_and_cancel_never_applies() {
        let mut state = loaded_state();
        state.action(Action::CleanWorktrees);
        state.apply(Outcome::WorktreeGc {
            apply: false,
            result: Ok(gc_report(&[])),
        });
        assert!(state.key(press(KeyCode::Enter)).jobs.is_empty());
        assert_eq!(state.popup, None);

        state.action(Action::CleanWorktrees);
        state.apply(Outcome::WorktreeGc {
            apply: false,
            result: Ok(gc_report(&["a-worktrees/K-1  pr=merged  1MB"])),
        });
        assert!(state.key(press(KeyCode::Esc)).jobs.is_empty());
        assert_eq!(state.popup, None);

        state.action(Action::CleanWorktrees);
        state.apply(Outcome::WorktreeGc {
            apply: false,
            result: Err("vk-wt: comando não encontrado".into()),
        });
        assert_eq!(
            state.popup,
            Some(Popup::WorktreeGc {
                phase: GcPhase::Failed("vk-wt: comando não encontrado".into()),
                scroll: 0
            })
        );
        assert!(state.action(Action::Confirm).jobs.is_empty());
        assert_eq!(state.popup, None);
    }

    #[test]
    fn apply_result_after_closing_the_popup_becomes_a_notice_and_refreshes() {
        let mut state = loaded_state();
        state.action(Action::CleanWorktrees);
        state.apply(Outcome::WorktreeGc {
            apply: false,
            result: Ok(gc_report(&["a-worktrees/K-1  pr=merged  1MB"])),
        });
        state.action(Action::Confirm);
        state.action(Action::Close);
        state.loading = false;
        let effects = state.apply(Outcome::WorktreeGc {
            apply: true,
            result: Ok(GcReport {
                summary: Some("1 worktrees removidas, 0 orfas podadas.".into()),
                ..GcReport::default()
            }),
        });
        assert!(effects.jobs.contains(&Job::Refresh));
        assert_eq!(
            state.notice.as_ref().map(|notice| notice.text.as_str()),
            Some("1 worktrees removidas, 0 orfas podadas.")
        );
        assert_eq!(state.popup, None);
    }

    #[test]
    fn stale_dry_run_results_do_not_reopen_a_closed_popup() {
        let mut state = loaded_state();
        state.action(Action::CleanWorktrees);
        state.action(Action::Close);
        state.apply(Outcome::WorktreeGc {
            apply: false,
            result: Ok(gc_report(&["a-worktrees/K-1  pr=merged  1MB"])),
        });
        assert_eq!(state.popup, None);
    }
}
