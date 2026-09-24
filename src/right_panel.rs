use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::config::{RightPanelConfig, RightPanelTabConfig};

use crate::layout::PaneId;
use crate::popup_size::PopupSize;
use crate::terminal::TerminalId;

pub(crate) const PUBLIC_ID_PREFIX: &str = "right-panel:";
pub(crate) const MAX_INSTANCES: usize = 16;
pub(crate) const FAILED_START_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
const MIN_PANEL_COLS: u16 = 20;
const MIN_TAB_COLS: u16 = 20;
pub(crate) const PANEL_OWNER_PANE_ENV: &str = "HERDR_PANEL_OWNER_PANE_ID";
pub(crate) const SESSION_REPOS_ENV: &str = "HERDR_SESSION_REPOS";
const DEFAULT_WIDTH_PERCENT: u8 = 45;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum RightPanelMode {
    #[default]
    Files,
    Diff,
    Jira,
    Custom(String),
}

impl RightPanelMode {
    pub(crate) const BUILTIN: [RightPanelMode; 3] = [
        RightPanelMode::Files,
        RightPanelMode::Diff,
        RightPanelMode::Jira,
    ];

    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Files => "files",
            Self::Diff => "diff",
            Self::Jira => "jira",
            Self::Custom(label) => label,
        }
    }

    pub(crate) fn from_label(label: &str) -> Self {
        Self::BUILTIN
            .into_iter()
            .find(|mode| mode.label() == label)
            .unwrap_or_else(|| Self::Custom(label.to_owned()))
    }
}

impl Serialize for RightPanelMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for RightPanelMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let label = String::deserialize(deserializer)?;
        Ok(Self::from_label(&label))
    }
}

impl schemars::JsonSchema for RightPanelMode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RightPanelMode".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "anyOf": [
                {
                    "type": "string",
                    "enum": ["files", "diff", "jira"]
                },
                {
                    "type": "string",
                    "description": "Label of a [[right_panel.tabs]] entry."
                }
            ]
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RightPanelCycleDirection {
    Next,
    Previous,
}

pub(crate) fn default_width() -> PopupSize {
    PopupSize::Percent(DEFAULT_WIDTH_PERCENT)
}

pub(crate) fn split_area(area: Rect, width: PopupSize) -> Option<(Rect, Rect)> {
    if area.width < MIN_PANEL_COLS + MIN_TAB_COLS || area.height < 3 {
        return None;
    }
    let panel_cols = width
        .resolve(area.width)
        .clamp(MIN_PANEL_COLS, area.width - MIN_TAB_COLS);
    let tab = Rect::new(area.x, area.y, area.width - panel_cols, area.height);
    let panel = Rect::new(tab.x + tab.width, area.y, panel_cols, area.height);
    Some((tab, panel))
}

pub(crate) fn content_rect(panel: Rect) -> Rect {
    Rect::new(
        panel.x.saturating_add(1),
        panel.y.saturating_add(1),
        panel.width.saturating_sub(1),
        panel.height.saturating_sub(1),
    )
}

pub(crate) fn header_modes(custom_labels: &[String]) -> Vec<RightPanelMode> {
    RightPanelMode::BUILTIN
        .into_iter()
        .chain(
            custom_labels
                .iter()
                .map(|label| RightPanelMode::Custom(label.clone())),
        )
        .collect()
}

pub(crate) fn header_tabs(panel: Rect, modes: &[RightPanelMode]) -> Vec<(RightPanelMode, Rect)> {
    let mut x = panel.x.saturating_add(2);
    let right = panel.x.saturating_add(panel.width);
    let mut tabs = Vec::new();
    for mode in modes {
        let width = u16::try_from(mode.label().chars().count())
            .unwrap_or(u16::MAX)
            .saturating_add(2);
        if x.saturating_add(width) > right {
            break;
        }
        tabs.push((mode.clone(), Rect::new(x, panel.y, width, 1)));
        x = x.saturating_add(width + 1);
    }
    tabs
}

pub(crate) fn public_id(terminal_id: &TerminalId) -> String {
    format!("{PUBLIC_ID_PREFIX}{terminal_id}")
}

pub(crate) fn parse_public_id(id: &str) -> Option<&str> {
    id.strip_prefix(PUBLIC_ID_PREFIX)
}

pub(crate) fn resolve_dir(
    worktree: Option<&Path>,
    pane_cwd: Option<PathBuf>,
    home: Option<PathBuf>,
) -> PathBuf {
    worktree
        .filter(|path| path.is_dir())
        .map(Path::to_path_buf)
        .or(pane_cwd)
        .or(home)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RightPanelInstance {
    pub owner: PaneId,
    pub mode: RightPanelMode,
    pub dir: PathBuf,
    pub pane_id: PaneId,
    pub terminal_id: TerminalId,
    pub spawned_at: std::time::Instant,
    pub exited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingOpen {
    pub dir: PathBuf,
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PanelExit {
    Released(RightPanelInstance),
    KeptFailedStart,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PanePanel {
    pub visible: bool,
    pub focused: bool,
    pub mode: RightPanelMode,
    pub pending_open: Option<PendingOpen>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RightPanelState {
    pub width: PopupSize,
    pub manual_width: Option<u16>,
    pub files_command: String,
    pub diff_command: String,
    pub open_command: String,
    pub jira_command: String,
    pub custom_tabs: Vec<RightPanelTabConfig>,
    pub panes: std::collections::HashMap<PaneId, PanePanel>,
    pub instances: Vec<RightPanelInstance>,
}

impl Default for RightPanelState {
    fn default() -> Self {
        let config = RightPanelConfig::default();
        Self {
            width: default_width(),
            manual_width: None,
            files_command: config.files_command,
            diff_command: config.diff_command,
            open_command: config.open_command,
            jira_command: config.jira_command,
            custom_tabs: Vec::new(),
            panes: std::collections::HashMap::new(),
            instances: Vec::new(),
        }
    }
}

impl RightPanelState {
    pub(crate) fn apply_config(
        &mut self,
        width: PopupSize,
        config: &RightPanelConfig,
    ) -> Vec<RightPanelInstance> {
        self.width = width;
        self.files_command = config.files_command.clone();
        self.diff_command = config.diff_command.clone();
        self.open_command = config.open_command.clone();
        self.jira_command = config.jira_command.clone();
        self.custom_tabs = config.resolved_tabs().0;
        let known = self.modes();
        for pane in self.panes.values_mut() {
            if !known.contains(&pane.mode) {
                pane.mode = RightPanelMode::Files;
            }
        }
        let (kept, removed) = std::mem::take(&mut self.instances)
            .into_iter()
            .partition(|instance| known.contains(&instance.mode));
        self.instances = kept;
        removed
    }

    pub(crate) fn custom_labels(&self) -> Vec<String> {
        self.custom_tabs
            .iter()
            .map(|tab| tab.label.clone())
            .collect()
    }

    pub(crate) fn modes(&self) -> Vec<RightPanelMode> {
        header_modes(&self.custom_labels())
    }

    pub(crate) fn command(&self, mode: &RightPanelMode) -> Option<&str> {
        match mode {
            RightPanelMode::Files => Some(&self.files_command),
            RightPanelMode::Diff => Some(&self.diff_command),
            RightPanelMode::Jira => Some(&self.jira_command),
            RightPanelMode::Custom(label) => self
                .custom_tabs
                .iter()
                .find(|tab| &tab.label == label)
                .map(|tab| tab.command.as_str()),
        }
    }

    pub(crate) fn cycled(
        &self,
        mode: &RightPanelMode,
        direction: RightPanelCycleDirection,
    ) -> RightPanelMode {
        let modes = self.modes();
        let len = modes.len();
        let index = modes.iter().position(|known| known == mode).unwrap_or(0);
        let next = match direction {
            RightPanelCycleDirection::Next => (index + 1) % len,
            RightPanelCycleDirection::Previous => (index + len - 1) % len,
        };
        modes[next].clone()
    }

    pub(crate) fn effective_width(&self) -> PopupSize {
        self.manual_width.map_or(self.width, PopupSize::Cells)
    }

    pub(crate) fn pane(&self, owner: PaneId) -> Option<&PanePanel> {
        self.panes.get(&owner)
    }

    pub(crate) fn pane_mut(&mut self, owner: PaneId) -> &mut PanePanel {
        self.panes.entry(owner).or_default()
    }

    pub(crate) fn is_visible_for(&self, owner: Option<PaneId>) -> bool {
        owner
            .and_then(|owner| self.pane(owner))
            .is_some_and(|pane| pane.visible)
    }

    pub(crate) fn any_visible(&self) -> bool {
        self.panes.values().any(|pane| pane.visible)
    }

    pub(crate) fn tab_area(&self, owner: Option<PaneId>, area: Rect) -> Rect {
        if !self.is_visible_for(owner) {
            return area;
        }
        split_area(area, self.effective_width()).map_or(area, |(tab, _)| tab)
    }

    pub(crate) fn panel_rect(&self, owner: Option<PaneId>, area: Rect) -> Option<Rect> {
        if !self.is_visible_for(owner) {
            return None;
        }
        split_area(area, self.effective_width()).map(|(_, panel)| panel)
    }

    pub(crate) fn instance_for(&self, owner: PaneId, mode: &RightPanelMode) -> Option<usize> {
        self.instances
            .iter()
            .position(|instance| instance.owner == owner && &instance.mode == mode)
    }

    pub(crate) fn displayed(&self, owner: PaneId) -> Option<&RightPanelInstance> {
        let pane = self.pane(owner).filter(|pane| pane.visible)?;
        self.instance_for(owner, &pane.mode)
            .map(|index| &self.instances[index])
    }

    pub(crate) fn owns_terminal(&self, terminal_id: &str) -> Option<&RightPanelInstance> {
        self.instances
            .iter()
            .find(|instance| instance.terminal_id.as_str() == terminal_id)
    }

    pub(crate) fn touch(&mut self, index: usize) -> TerminalId {
        let instance = self.instances.remove(index);
        let terminal_id = instance.terminal_id.clone();
        self.instances.push(instance);
        terminal_id
    }

    pub(crate) fn evict_over(
        &mut self,
        cap: usize,
        protect: &[TerminalId],
    ) -> Vec<RightPanelInstance> {
        let mut evicted = Vec::new();
        while self.instances.len() > cap {
            let Some(index) = self
                .instances
                .iter()
                .position(|instance| !protect.contains(&instance.terminal_id))
            else {
                break;
            };
            evicted.push(self.instances.remove(index));
        }
        evicted
    }

    pub(crate) fn take_exited_for(&mut self, owner: PaneId) -> Vec<RightPanelInstance> {
        let (exited, live) = std::mem::take(&mut self.instances)
            .into_iter()
            .partition(|instance| instance.owner == owner && instance.exited);
        self.instances = live;
        exited
    }

    pub(crate) fn take_instance(
        &mut self,
        owner: PaneId,
        mode: &RightPanelMode,
    ) -> Option<RightPanelInstance> {
        self.instance_for(owner, mode)
            .map(|index| self.instances.remove(index))
    }

    pub(crate) fn remove_owner(&mut self, owner: PaneId) -> Vec<RightPanelInstance> {
        self.panes.remove(&owner);
        let (removed, kept) = std::mem::take(&mut self.instances)
            .into_iter()
            .partition(|instance| instance.owner == owner);
        self.instances = kept;
        removed
    }

    pub(crate) fn command_exited(
        &mut self,
        pane_id: PaneId,
        now: std::time::Instant,
    ) -> Option<PanelExit> {
        let index = self
            .instances
            .iter()
            .position(|instance| instance.pane_id == pane_id)?;
        let instance = &self.instances[index];
        let owner = instance.owner;
        let displayed = self
            .pane(owner)
            .is_some_and(|pane| pane.visible && pane.mode == instance.mode);
        let failed_start = now.saturating_duration_since(instance.spawned_at) < FAILED_START_WINDOW;
        if displayed && failed_start {
            self.instances[index].exited = true;
            return Some(PanelExit::KeptFailedStart);
        }
        let instance = self.instances.remove(index);
        if displayed {
            let pane = self.pane_mut(owner);
            pane.visible = false;
            pane.focused = false;
        }
        Some(PanelExit::Released(instance))
    }

    pub(crate) fn focused_public_id(&self, owner: Option<PaneId>) -> Option<String> {
        let owner = owner?;
        if !self.pane(owner).is_some_and(|pane| pane.focused) {
            return None;
        }
        self.displayed(owner)
            .map(|instance| public_id(&instance.terminal_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenTarget {
    File { path: PathBuf, line: u32 },
    Dir(PathBuf),
}

impl OpenTarget {
    pub(crate) fn dir(&self) -> PathBuf {
        match self {
            Self::File { path, .. } => path
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf),
            Self::Dir(dir) => dir.clone(),
        }
    }
}

pub(crate) fn resolve_open_target(
    raw: &str,
    line: Option<u32>,
    base: &Path,
) -> Result<OpenTarget, String> {
    let absolute = |text: &str| {
        let path = Path::new(text);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    };
    let literal = absolute(raw);
    let (path, line) = if literal.exists() || line.is_some() {
        (literal, line)
    } else if let Some((path, parsed)) = split_path_line(raw) {
        (absolute(path), Some(parsed))
    } else {
        (literal, None)
    };
    if path.is_dir() {
        return Ok(OpenTarget::Dir(path));
    }
    if !path.is_file() {
        return Err(format!("file not found: {}", path.display()));
    }
    Ok(OpenTarget::File {
        path,
        line: line.unwrap_or(1).max(1),
    })
}

fn split_path_line(raw: &str) -> Option<(&str, u32)> {
    let (path, line) = raw.rsplit_once(':')?;
    let line = line.parse::<u32>().ok()?;
    (!path.is_empty()).then_some((path, line))
}

pub(crate) fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub(crate) fn render_open_command(template: &str, path: &Path, line: u32) -> String {
    let dir = path.parent().unwrap_or_else(|| Path::new("/"));
    template
        .replace("{path}", &shell_quote(&path.to_string_lossy()))
        .replace("{dir}", &shell_quote(&dir.to_string_lossy()))
        .replace("{line}", &line.max(1).to_string())
}

pub(crate) fn worktree_path(text: &str) -> Option<String> {
    const MARKER: &str = "-worktrees/";
    let marker = text.rfind(MARKER)?;
    let start = text[..marker]
        .rfind(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | '(' | '`'))
        .map_or(0, |index| index + 1);
    let name_start = marker + MARKER.len();
    let name_len = text[name_start..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(text.len() - name_start);
    let name = text[name_start..name_start + name_len].trim_end_matches('.');
    let prefix = &text[start..marker];
    if name.is_empty() || !prefix.starts_with('/') {
        return None;
    }
    Some(format!("{prefix}{MARKER}{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(owner: PaneId, mode: RightPanelMode, dir: &str) -> RightPanelInstance {
        RightPanelInstance {
            owner,
            mode,
            dir: PathBuf::from(dir),
            pane_id: PaneId::alloc(),
            terminal_id: TerminalId::alloc(),
            spawned_at: std::time::Instant::now(),
            exited: false,
        }
    }

    #[test]
    fn split_area_resolves_percent_and_cells_with_clamps() {
        let area = Rect::new(0, 0, 100, 30);
        let (tab, panel) = split_area(area, PopupSize::Percent(45)).unwrap();
        assert_eq!(panel, Rect::new(55, 0, 45, 30));
        assert_eq!(tab, Rect::new(0, 0, 55, 30));

        let (_, panel) = split_area(area, PopupSize::Cells(5)).unwrap();
        assert_eq!(panel.width, MIN_PANEL_COLS);
        let (tab, panel) = split_area(area, PopupSize::Cells(95)).unwrap();
        assert_eq!(tab.width, MIN_TAB_COLS);
        assert_eq!(panel.width, 80);

        assert_eq!(split_area(Rect::new(0, 0, 39, 30), default_width()), None);
    }

    #[test]
    fn content_and_header_tabs_follow_the_panel_rect() {
        let panel = Rect::new(60, 0, 40, 20);
        assert_eq!(content_rect(panel), Rect::new(61, 1, 39, 19));
        let tabs = header_tabs(panel, &header_modes(&[]));
        assert_eq!(
            tabs,
            vec![
                (RightPanelMode::Files, Rect::new(62, 0, 7, 1)),
                (RightPanelMode::Diff, Rect::new(70, 0, 6, 1)),
                (RightPanelMode::Jira, Rect::new(77, 0, 6, 1)),
            ]
        );
    }

    #[test]
    fn public_ids_round_trip() {
        let terminal_id = TerminalId::alloc();
        let id = public_id(&terminal_id);
        assert_eq!(parse_public_id(&id), Some(terminal_id.as_str()));
        assert_eq!(parse_public_id("w1:p1"), None);
    }

    #[test]
    fn resolve_dir_prefers_existing_worktree_then_pane_cwd() {
        let existing = std::env::temp_dir();
        assert_eq!(
            resolve_dir(Some(&existing), Some("/pane".into()), None),
            existing
        );
        assert_eq!(
            resolve_dir(
                Some(Path::new("/nonexistent/app-worktrees/VK-1")),
                Some("/pane".into()),
                Some("/home".into())
            ),
            PathBuf::from("/pane")
        );
        assert_eq!(
            resolve_dir(None, None, Some("/home".into())),
            PathBuf::from("/home")
        );
    }

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "herdr-right-panel-{}-{}",
                std::process::id(),
                TerminalId::alloc()
            ));
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src/a.rs"), "fn main() {}\n").unwrap();
            std::fs::write(root.join("with space's.txt"), "x\n").unwrap();
            Self(root)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn open_target_resolves_relative_absolute_and_line_shorthand() {
        let tree = TempTree::new();
        let file = tree.0.join("src/a.rs");

        assert_eq!(
            resolve_open_target("src/a.rs", None, &tree.0),
            Ok(OpenTarget::File {
                path: file.clone(),
                line: 1
            })
        );
        assert_eq!(
            resolve_open_target(file.to_str().unwrap(), Some(9), Path::new("/elsewhere")),
            Ok(OpenTarget::File {
                path: file.clone(),
                line: 9
            })
        );
        assert_eq!(
            resolve_open_target("src/a.rs:42", None, &tree.0),
            Ok(OpenTarget::File {
                path: file.clone(),
                line: 42
            })
        );
        assert_eq!(
            resolve_open_target("src/a.rs", Some(0), &tree.0),
            Ok(OpenTarget::File {
                path: file.clone(),
                line: 1
            })
        );
        assert_eq!(
            resolve_open_target("src", None, &tree.0),
            Ok(OpenTarget::Dir(tree.0.join("src")))
        );
        assert!(resolve_open_target("src/missing.rs", None, &tree.0).is_err());
        assert!(resolve_open_target("src/a.rs:42", Some(3), &tree.0).is_err());
        assert!(resolve_open_target("src/a.rs:x", None, &tree.0).is_err());
    }

    #[test]
    fn open_target_dir_is_the_file_parent() {
        let target = OpenTarget::File {
            path: PathBuf::from("/r/src/a.rs"),
            line: 3,
        };
        assert_eq!(target.dir(), PathBuf::from("/r/src"));
        assert_eq!(
            OpenTarget::Dir(PathBuf::from("/r")).dir(),
            PathBuf::from("/r")
        );
    }

    #[test]
    fn open_command_quotes_paths_and_defaults_line() {
        let tree = TempTree::new();
        let path = tree.0.join("with space's.txt");
        let quoted = shell_quote(path.to_str().unwrap());

        let rendered = render_open_command(&RightPanelConfig::default().open_command, &path, 0);

        assert_eq!(rendered, format!("micro +1 {quoted}; exec yazi {quoted}"));
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(
            render_open_command(
                "cd {dir} && vim +{line} {path}",
                Path::new("/r/a b/c.rs"),
                7
            ),
            "cd '/r/a b' && vim +7 '/r/a b/c.rs'"
        );
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(render_open_command("printf %s {path}", &path, 1))
            .output()
            .unwrap();
        assert_eq!(output.stdout, path.to_str().unwrap().as_bytes());
    }

    #[test]
    fn worktree_path_extracts_absolute_prefix_from_paths_and_commands() {
        assert_eq!(
            worktree_path("/r/api-worktrees/VK25-1-api/src/a.rb").as_deref(),
            Some("/r/api-worktrees/VK25-1-api")
        );
        assert_eq!(
            worktree_path("cd /r/web-worktrees/VK25-2904 && git status").as_deref(),
            Some("/r/web-worktrees/VK25-2904")
        );
        assert_eq!(
            worktree_path("git -C \"/r/web-worktrees/VK25-7.\"").as_deref(),
            Some("/r/web-worktrees/VK25-7")
        );
        assert_eq!(worktree_path("see web-worktrees/VK25-1"), None);
        assert_eq!(worktree_path("/r/web-worktrees/"), None);
    }

    fn shown(state: &mut RightPanelState, owner: PaneId, mode: RightPanelMode) {
        let pane = state.pane_mut(owner);
        pane.visible = true;
        pane.mode = mode;
    }

    #[test]
    fn layout_follows_the_owner_pane_visibility() {
        let area = Rect::new(0, 0, 120, 40);
        let (a, b) = (PaneId::alloc(), PaneId::alloc());
        let mut state = RightPanelState::default();
        shown(&mut state, a, RightPanelMode::Files);

        assert_eq!(state.tab_area(Some(a), area).width, 66);
        assert_eq!(
            state.panel_rect(Some(a), area),
            Some(Rect::new(66, 0, 54, 40))
        );
        assert_eq!(state.tab_area(Some(b), area), area);
        assert_eq!(state.panel_rect(Some(b), area), None);
        assert_eq!(state.tab_area(None, area), area);
        assert!(state.any_visible());
    }

    #[test]
    fn manual_width_is_shared_by_every_pane_within_the_clamps() {
        let area = Rect::new(0, 0, 120, 40);
        let (a, b) = (PaneId::alloc(), PaneId::alloc());
        let mut state = RightPanelState::default();
        shown(&mut state, a, RightPanelMode::Files);
        shown(&mut state, b, RightPanelMode::Diff);
        state.manual_width = Some(30);
        assert_eq!(state.panel_rect(Some(a), area).unwrap().width, 30);
        assert_eq!(state.panel_rect(Some(b), area).unwrap().width, 30);
        state.manual_width = Some(5);
        assert_eq!(
            state.panel_rect(Some(a), area).unwrap().width,
            MIN_PANEL_COLS
        );
        state.manual_width = Some(500);
        assert_eq!(state.tab_area(Some(a), area).width, MIN_TAB_COLS);
        state.manual_width = None;
        assert_eq!(state.panel_rect(Some(a), area).unwrap().width, 54);
    }

    #[test]
    fn displayed_instance_is_the_owner_instance_for_its_mode() {
        let (a, b) = (PaneId::alloc(), PaneId::alloc());
        let mut state = RightPanelState {
            instances: vec![
                instance(a, RightPanelMode::Files, "/a"),
                instance(a, RightPanelMode::Diff, "/a"),
                instance(b, RightPanelMode::Files, "/a"),
            ],
            ..Default::default()
        };
        assert_eq!(state.displayed(a), None);
        shown(&mut state, a, RightPanelMode::Diff);

        assert_eq!(state.displayed(a), Some(&state.instances[1]));
        state.pane_mut(a).mode = RightPanelMode::Files;
        assert_eq!(state.displayed(a), Some(&state.instances[0]));
        assert_eq!(state.displayed(b), None);
        assert_eq!(state.instance_for(b, &RightPanelMode::Files), Some(2));
        assert_eq!(state.instance_for(b, &RightPanelMode::Diff), None);
    }

    #[test]
    fn eviction_drops_least_recent_but_never_a_protected_instance() {
        let owner = PaneId::alloc();
        let mut state = RightPanelState {
            instances: (0..4)
                .map(|index| instance(owner, RightPanelMode::Files, &format!("/{index}")))
                .collect(),
            ..Default::default()
        };
        let protected = state.instances[0].terminal_id.clone();

        let evicted = state.evict_over(2, std::slice::from_ref(&protected));

        assert_eq!(
            evicted.iter().map(|i| i.dir.clone()).collect::<Vec<_>>(),
            [PathBuf::from("/1"), PathBuf::from("/2")]
        );
        assert!(state.owns_terminal(protected.as_str()).is_some());
        assert_eq!(MAX_INSTANCES, 16);
    }

    #[test]
    fn touch_moves_an_instance_to_most_recent() {
        let owner = PaneId::alloc();
        let mut state = RightPanelState {
            instances: vec![
                instance(owner, RightPanelMode::Files, "/a"),
                instance(owner, RightPanelMode::Diff, "/a"),
            ],
            ..Default::default()
        };
        let first = state.instances[0].terminal_id.clone();

        assert_eq!(state.touch(0), first);
        assert_eq!(state.instances[1].terminal_id, first);
    }

    #[test]
    fn exit_is_scoped_to_the_owner_pane() {
        let (a, b) = (PaneId::alloc(), PaneId::alloc());
        let mut state = RightPanelState {
            instances: vec![
                instance(a, RightPanelMode::Files, "/a"),
                instance(b, RightPanelMode::Files, "/b"),
            ],
            ..Default::default()
        };
        shown(&mut state, a, RightPanelMode::Files);
        shown(&mut state, b, RightPanelMode::Files);
        state.pane_mut(a).focused = true;
        let later = std::time::Instant::now() + FAILED_START_WINDOW;

        let a_pane = state.instances[0].pane_id;
        assert!(matches!(
            state.command_exited(a_pane, later),
            Some(PanelExit::Released(_))
        ));

        assert!(!state.pane(a).unwrap().visible && !state.pane(a).unwrap().focused);
        assert!(state.pane(b).unwrap().visible);
        assert!(state.displayed(b).is_some());
        assert_eq!(state.command_exited(a_pane, later), None);
    }

    #[test]
    fn early_exit_keeps_the_notice_until_the_owner_shows_again() {
        let owner = PaneId::alloc();
        let mut state = RightPanelState {
            instances: vec![
                instance(owner, RightPanelMode::Files, "/a"),
                instance(owner, RightPanelMode::Diff, "/a"),
            ],
            ..Default::default()
        };
        shown(&mut state, owner, RightPanelMode::Diff);
        let diff = state.instances[1].pane_id;

        assert_eq!(
            state.command_exited(diff, std::time::Instant::now()),
            Some(PanelExit::KeptFailedStart)
        );
        assert!(state.pane(owner).unwrap().visible);
        assert!(state.displayed(owner).unwrap().exited);

        let released = state.take_exited_for(owner);
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].pane_id, diff);
        assert_eq!(state.instances.len(), 1);
    }

    #[test]
    fn background_instance_exit_does_not_hide_the_panel() {
        let owner = PaneId::alloc();
        let mut state = RightPanelState {
            instances: vec![
                instance(owner, RightPanelMode::Files, "/a"),
                instance(owner, RightPanelMode::Diff, "/a"),
            ],
            ..Default::default()
        };
        shown(&mut state, owner, RightPanelMode::Files);
        let diff = state.instances[1].pane_id;

        assert!(matches!(
            state.command_exited(diff, std::time::Instant::now()),
            Some(PanelExit::Released(_))
        ));
        assert!(state.pane(owner).unwrap().visible);
    }

    #[test]
    fn removing_an_owner_releases_only_its_instances() {
        let (a, b) = (PaneId::alloc(), PaneId::alloc());
        let mut state = RightPanelState {
            instances: vec![
                instance(a, RightPanelMode::Files, "/a"),
                instance(b, RightPanelMode::Files, "/b"),
                instance(a, RightPanelMode::Diff, "/a"),
            ],
            ..Default::default()
        };
        shown(&mut state, a, RightPanelMode::Files);

        let removed = state.remove_owner(a);

        assert_eq!(removed.len(), 2);
        assert!(state.pane(a).is_none());
        assert_eq!(state.instances.len(), 1);
        assert_eq!(state.instances[0].owner, b);
    }

    #[test]
    fn focused_public_id_requires_the_owner_panel_focused_and_shown() {
        let owner = PaneId::alloc();
        let mut state = RightPanelState {
            instances: vec![instance(owner, RightPanelMode::Files, "/a")],
            ..Default::default()
        };
        assert_eq!(state.focused_public_id(Some(owner)), None);
        shown(&mut state, owner, RightPanelMode::Files);
        assert_eq!(state.focused_public_id(Some(owner)), None);
        state.pane_mut(owner).focused = true;
        assert_eq!(
            state.focused_public_id(Some(owner)),
            Some(public_id(&state.instances[0].terminal_id))
        );
        assert_eq!(state.focused_public_id(None), None);
    }
}
