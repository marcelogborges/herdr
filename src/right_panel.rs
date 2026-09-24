use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::config::RightPanelConfig;

use crate::layout::PaneId;
use crate::popup_size::PopupSize;
use crate::terminal::TerminalId;

pub(crate) const PUBLIC_ID_PREFIX: &str = "right-panel:";
pub(crate) const MAX_INSTANCES: usize = 8;
pub(crate) const FAILED_START_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
const MIN_PANEL_COLS: u16 = 20;
const MIN_TAB_COLS: u16 = 20;
const DEFAULT_WIDTH_PERCENT: u8 = 45;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RightPanelMode {
    #[default]
    Files,
    Diff,
}

impl RightPanelMode {
    pub(crate) const ALL: [RightPanelMode; 2] = [RightPanelMode::Files, RightPanelMode::Diff];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Diff => "diff",
        }
    }
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

pub(crate) fn header_tabs(panel: Rect) -> Vec<(RightPanelMode, Rect)> {
    let mut x = panel.x.saturating_add(2);
    let right = panel.x.saturating_add(panel.width);
    let mut tabs = Vec::new();
    for mode in RightPanelMode::ALL {
        let width = mode.label().len() as u16 + 2;
        if x.saturating_add(width) > right {
            break;
        }
        tabs.push((mode, Rect::new(x, panel.y, width, 1)));
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RightPanelState {
    pub visible: bool,
    pub focused: bool,
    pub mode: RightPanelMode,
    pub width: PopupSize,
    pub manual_width: Option<u16>,
    pub files_command: String,
    pub diff_command: String,
    pub open_command: String,
    pub pending_open: Option<PendingOpen>,
    pub instances: Vec<RightPanelInstance>,
    pub active: Option<TerminalId>,
    pub target: Option<(usize, PaneId, RightPanelMode)>,
}

impl Default for RightPanelState {
    fn default() -> Self {
        let config = RightPanelConfig::default();
        Self {
            visible: false,
            focused: false,
            mode: RightPanelMode::Files,
            width: default_width(),
            manual_width: None,
            files_command: config.files_command,
            diff_command: config.diff_command,
            open_command: config.open_command,
            pending_open: None,
            instances: Vec::new(),
            active: None,
            target: None,
        }
    }
}

impl RightPanelState {
    pub(crate) fn apply_config(&mut self, width: PopupSize, config: &RightPanelConfig) {
        self.width = width;
        self.files_command = config.files_command.clone();
        self.diff_command = config.diff_command.clone();
        self.open_command = config.open_command.clone();
    }

    pub(crate) fn command(&self, mode: RightPanelMode) -> &str {
        match mode {
            RightPanelMode::Files => &self.files_command,
            RightPanelMode::Diff => &self.diff_command,
        }
    }

    pub(crate) fn tab_area(&self, area: Rect) -> Rect {
        if !self.visible {
            return area;
        }
        split_area(area, self.effective_width()).map_or(area, |(tab, _)| tab)
    }

    pub(crate) fn panel_rect(&self, area: Rect) -> Option<Rect> {
        if !self.visible {
            return None;
        }
        split_area(area, self.effective_width()).map(|(_, panel)| panel)
    }

    pub(crate) fn effective_width(&self) -> PopupSize {
        self.manual_width.map_or(self.width, PopupSize::Cells)
    }

    pub(crate) fn active_instance(&self) -> Option<&RightPanelInstance> {
        let active = self.active.as_ref()?;
        self.instances
            .iter()
            .find(|instance| &instance.terminal_id == active)
    }

    pub(crate) fn owns_terminal(&self, terminal_id: &str) -> Option<&RightPanelInstance> {
        self.instances
            .iter()
            .find(|instance| instance.terminal_id.as_str() == terminal_id)
    }

    pub(crate) fn find(&self, mode: RightPanelMode, dir: &Path) -> Option<usize> {
        self.instances
            .iter()
            .position(|instance| !instance.exited && instance.mode == mode && instance.dir == dir)
    }

    pub(crate) fn take_exited(&mut self) -> Vec<RightPanelInstance> {
        let (exited, live) = std::mem::take(&mut self.instances)
            .into_iter()
            .partition(|instance| instance.exited);
        self.instances = live;
        if self.active_instance().is_none() {
            self.active = None;
            self.target = None;
        }
        exited
    }

    pub(crate) fn activate(&mut self, index: usize) {
        let instance = self.instances.remove(index);
        self.active = Some(instance.terminal_id.clone());
        self.instances.push(instance);
    }

    pub(crate) fn evict_over(&mut self, cap: usize) -> Vec<RightPanelInstance> {
        let mut evicted = Vec::new();
        while self.instances.len() > cap {
            let Some(index) = self
                .instances
                .iter()
                .position(|instance| Some(&instance.terminal_id) != self.active.as_ref())
            else {
                break;
            };
            evicted.push(self.instances.remove(index));
        }
        evicted
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
        let is_active = self.active.as_ref() == Some(&self.instances[index].terminal_id);
        let failed_start =
            now.saturating_duration_since(self.instances[index].spawned_at) < FAILED_START_WINDOW;
        if is_active && self.visible && failed_start {
            self.instances[index].exited = true;
            return Some(PanelExit::KeptFailedStart);
        }
        let instance = self.instances.remove(index);
        if is_active {
            self.active = None;
            self.target = None;
            self.visible = false;
            self.focused = false;
        }
        Some(PanelExit::Released(instance))
    }

    pub(crate) fn focused_public_id(&self) -> Option<String> {
        if !self.visible || !self.focused {
            return None;
        }
        self.active.as_ref().map(public_id)
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

    fn instance(mode: RightPanelMode, dir: &str) -> RightPanelInstance {
        RightPanelInstance {
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
    fn tab_area_shrinks_only_while_visible() {
        let area = Rect::new(0, 0, 120, 40);
        let mut state = RightPanelState::default();
        assert_eq!(state.tab_area(area), area);
        assert_eq!(state.panel_rect(area), None);
        state.visible = true;
        assert_eq!(state.tab_area(area).width, 66);
        assert_eq!(state.panel_rect(area), Some(Rect::new(66, 0, 54, 40)));
    }

    #[test]
    fn manual_width_overrides_config_within_the_clamps() {
        let area = Rect::new(0, 0, 120, 40);
        let mut state = RightPanelState {
            visible: true,
            ..Default::default()
        };
        state.manual_width = Some(30);
        assert_eq!(state.panel_rect(area).unwrap().width, 30);
        assert_eq!(state.tab_area(area).width, 90);
        state.manual_width = Some(5);
        assert_eq!(state.panel_rect(area).unwrap().width, MIN_PANEL_COLS);
        state.manual_width = Some(500);
        assert_eq!(state.tab_area(area).width, MIN_TAB_COLS);
        state.manual_width = None;
        assert_eq!(state.panel_rect(area).unwrap().width, 54);
    }

    #[test]
    fn content_and_header_tabs_follow_the_panel_rect() {
        let panel = Rect::new(60, 0, 40, 20);
        assert_eq!(content_rect(panel), Rect::new(61, 1, 39, 19));
        let tabs = header_tabs(panel);
        assert_eq!(
            tabs,
            vec![
                (RightPanelMode::Files, Rect::new(62, 0, 7, 1)),
                (RightPanelMode::Diff, Rect::new(70, 0, 6, 1)),
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

    #[test]
    fn instances_are_keyed_by_mode_and_dir_and_touched_on_activate() {
        let mut state = RightPanelState {
            instances: vec![
                instance(RightPanelMode::Files, "/a"),
                instance(RightPanelMode::Diff, "/a"),
                instance(RightPanelMode::Files, "/b"),
            ],
            ..Default::default()
        };
        assert_eq!(state.find(RightPanelMode::Diff, Path::new("/a")), Some(1));
        assert_eq!(state.find(RightPanelMode::Diff, Path::new("/b")), None);

        state.activate(0);
        assert_eq!(state.instances.last().unwrap().dir, PathBuf::from("/a"));
        assert_eq!(state.active_instance().unwrap().mode, RightPanelMode::Files);
    }

    #[test]
    fn eviction_drops_least_recent_but_keeps_active() {
        let mut state = RightPanelState {
            instances: (0..4)
                .map(|index| instance(RightPanelMode::Files, &format!("/{index}")))
                .collect(),
            ..Default::default()
        };
        state.active = Some(state.instances[0].terminal_id.clone());

        let evicted = state.evict_over(2);

        assert_eq!(
            evicted.iter().map(|i| i.dir.clone()).collect::<Vec<_>>(),
            [PathBuf::from("/1"), PathBuf::from("/2")]
        );
        assert_eq!(state.instances.len(), 2);
        assert!(state.active_instance().is_some());
    }

    #[test]
    fn exit_of_active_instance_hides_the_panel() {
        let mut state = RightPanelState {
            instances: vec![
                instance(RightPanelMode::Files, "/a"),
                instance(RightPanelMode::Diff, "/a"),
            ],
            ..Default::default()
        };
        state.activate(1);
        state.visible = true;
        state.focused = true;
        let later = std::time::Instant::now() + FAILED_START_WINDOW;

        let background = state.instances[0].pane_id;
        assert!(matches!(
            state.command_exited(background, later),
            Some(PanelExit::Released(_))
        ));
        assert!(state.visible);

        let active = state.active_instance().unwrap().pane_id;
        assert!(matches!(
            state.command_exited(active, later),
            Some(PanelExit::Released(_))
        ));
        assert!(!state.visible && !state.focused);
        assert_eq!(state.active, None);
        assert!(state.instances.is_empty());
        assert_eq!(state.command_exited(active, later), None);
    }

    #[test]
    fn failed_start_keeps_the_panel_open_until_the_next_show() {
        let mut state = RightPanelState {
            instances: vec![
                instance(RightPanelMode::Files, "/a"),
                instance(RightPanelMode::Diff, "/a"),
            ],
            ..Default::default()
        };
        state.activate(1);
        state.visible = true;
        state.focused = true;
        let diff = state.active_instance().unwrap().pane_id;

        assert_eq!(
            state.command_exited(diff, std::time::Instant::now()),
            Some(PanelExit::KeptFailedStart)
        );
        assert!(state.visible);
        assert!(state.active_instance().unwrap().exited);
        assert_eq!(state.find(RightPanelMode::Diff, Path::new("/a")), None);
        assert_eq!(state.find(RightPanelMode::Files, Path::new("/a")), Some(0));

        let released = state.take_exited();

        assert_eq!(released.len(), 1);
        assert_eq!(released[0].pane_id, diff);
        assert_eq!(state.active, None);
        assert_eq!(state.target, None);
        assert_eq!(state.instances.len(), 1);
    }

    #[test]
    fn focused_public_id_requires_visible_focused_active() {
        let mut state = RightPanelState {
            instances: vec![instance(RightPanelMode::Files, "/a")],
            ..Default::default()
        };
        state.activate(0);
        assert_eq!(state.focused_public_id(), None);
        state.visible = true;
        state.focused = true;
        assert_eq!(
            state.focused_public_id(),
            Some(public_id(state.active.as_ref().unwrap()))
        );
    }
}
