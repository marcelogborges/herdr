use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use unicode_width::UnicodeWidthStr;

use crate::config::{RightPanelActivityBarConfig, RightPanelConfig, RightPanelTabConfig};

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
pub(crate) const ACTIVITY_BAR_COLS: u16 = 3;
pub(crate) const PROD_TAG: &str = "prod";
const HEADER_TAB_GAP: u16 = 2;
const PROD_BADGE: &str = " PROD ";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum RightPanelMode {
    #[default]
    Files,
    Custom(String),
}

impl RightPanelMode {
    pub(crate) const BUILTIN: [RightPanelMode; 1] = [RightPanelMode::Files];

    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Files => "files",
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
                    "enum": ["files"]
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

pub(crate) fn content_rect(panel: Rect, header_rows: u16) -> Rect {
    Rect::new(
        panel.x.saturating_add(1),
        panel.y.saturating_add(header_rows),
        panel.width.saturating_sub(1),
        panel.height.saturating_sub(header_rows),
    )
}

/// How the tab area, the panel, and the activity bar share a tab surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceSplit {
    pub tab: Rect,
    pub panel: Option<Rect>,
    pub bar: Option<Rect>,
}

pub(crate) fn surface_split(
    area: Rect,
    width: PopupSize,
    panel_visible: bool,
    bar_visible: bool,
) -> SurfaceSplit {
    let bar = bar_visible.then(|| activity_bar_rect(area)).flatten();
    let rest = bar.map_or(area, |bar| {
        Rect::new(area.x, area.y, bar.x - area.x, area.height)
    });
    let split = panel_visible.then(|| split_area(rest, width)).flatten();
    SurfaceSplit {
        tab: split.map_or(rest, |(tab, _)| tab),
        panel: split.map(|(_, panel)| panel),
        bar,
    }
}

/// The activity bar's columns at the right edge of a tab surface, when it fits.
pub(crate) fn activity_bar_rect(area: Rect) -> Option<Rect> {
    (area.width >= ACTIVITY_BAR_COLS + MIN_TAB_COLS).then(|| {
        Rect::new(
            area.x + area.width - ACTIVITY_BAR_COLS,
            area.y,
            ACTIVITY_BAR_COLS,
            area.height,
        )
    })
}

/// Rows of the activity bar's icons, one per view with a blank row between them.
pub(crate) fn activity_bar_icons(bar: Rect, views: usize) -> Vec<Rect> {
    (0..views)
        .map_while(|index| {
            let offset = u16::try_from(index).ok()?.checked_mul(2)?.checked_add(1)?;
            (offset < bar.height).then(|| Rect::new(bar.x, bar.y + offset, bar.width, 1))
        })
        .collect()
}

/// One tab of a right panel view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RightPanelViewTab {
    pub mode: RightPanelMode,
    pub label: String,
    #[serde(default)]
    pub prod: bool,
}

/// What the activity bar lists: the built-in files tab, one tab, or a group of tabs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RightPanelView {
    pub label: String,
    pub icon: String,
    /// Groups show a row of tabs under their title.
    #[serde(default)]
    pub group: bool,
    pub tabs: Vec<RightPanelViewTab>,
}

impl RightPanelView {
    pub(crate) fn header_rows(&self) -> u16 {
        if self.group {
            2
        } else {
            1
        }
    }

    pub(crate) fn contains(&self, mode: &RightPanelMode) -> bool {
        self.tabs.iter().any(|tab| &tab.mode == mode)
    }

    pub(crate) fn tab(&self, mode: &RightPanelMode) -> Option<&RightPanelViewTab> {
        self.tabs.iter().find(|tab| &tab.mode == mode)
    }
}

fn default_icon(label: &str) -> String {
    label
        .chars()
        .find(|c| !c.is_whitespace())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

pub(crate) fn build_views(
    files_icon: Option<&str>,
    tabs: &[RightPanelTabConfig],
) -> Vec<RightPanelView> {
    let files = RightPanelMode::Files;
    let mut views = vec![RightPanelView {
        label: files.label().to_owned(),
        icon: files_icon.map_or_else(|| default_icon(files.label()), str::to_owned),
        group: false,
        tabs: vec![RightPanelViewTab {
            label: files.label().to_owned(),
            mode: files,
            prod: false,
        }],
    }];
    for tab in tabs {
        let view_tab = RightPanelViewTab {
            mode: RightPanelMode::Custom(tab.label.clone()),
            label: tab.label.clone(),
            prod: tab.tag.as_deref() == Some(PROD_TAG),
        };
        let icon = || tab.icon.clone().unwrap_or_else(|| default_icon(&tab.label));
        match tab.group.as_deref() {
            Some(group) => {
                if let Some(view) = views
                    .iter_mut()
                    .find(|view| view.group && view.label == group)
                {
                    view.tabs.push(view_tab);
                } else {
                    views.push(RightPanelView {
                        label: group.to_owned(),
                        icon: icon(),
                        group: true,
                        tabs: vec![view_tab],
                    });
                }
            }
            None => views.push(RightPanelView {
                label: tab.label.clone(),
                icon: icon(),
                group: false,
                tabs: vec![view_tab],
            }),
        }
    }
    views
}

pub(crate) fn view_index(views: &[RightPanelView], mode: &RightPanelMode) -> Option<usize> {
    views.iter().position(|view| view.contains(mode))
}

fn text_width(text: &str) -> u16 {
    u16::try_from(text.width()).unwrap_or(u16::MAX)
}

/// Columns one header tab takes: state dot, space, label, and the optional PROD badge.
pub(crate) fn header_tab_width(tab: &RightPanelViewTab) -> u16 {
    let badge = if tab.prod {
        1 + text_width(PROD_BADGE)
    } else {
        0
    };
    text_width(&tab.label).saturating_add(2 + badge)
}

pub(crate) fn prod_badge() -> &'static str {
    PROD_BADGE
}

/// Clickable parts of a panel header, shared by the renderer and mouse hit-testing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct HeaderLayout {
    pub close: Option<Rect>,
    pub tabs: Vec<(RightPanelMode, Rect)>,
    /// Marker for tabs that do not fit: its rect, the tab it switches to, and how many are hidden.
    pub overflow: Option<(Rect, RightPanelMode, usize)>,
}

pub(crate) fn header_layout(
    panel: Rect,
    view: &RightPanelView,
    active: &RightPanelMode,
) -> HeaderLayout {
    let right = panel.x.saturating_add(panel.width);
    let close = (panel.width >= 8).then(|| Rect::new(right - 3, panel.y, 3, 1));
    if !view.group || panel.height < 2 {
        return HeaderLayout {
            close,
            ..HeaderLayout::default()
        };
    }
    let row = panel.y + 1;
    let left = panel.x.saturating_add(2);
    let limit = right.saturating_sub(1);
    let widths: Vec<u16> = view.tabs.iter().map(header_tab_width).collect();
    let fit = |start: usize, limit: u16| {
        let mut x = left;
        let mut end = start;
        for width in &widths[start..] {
            if x.saturating_add(*width) > limit {
                break;
            }
            x = x.saturating_add(*width + HEADER_TAB_GAP);
            end += 1;
        }
        end
    };
    let count = widths.len();
    let mut start = 0;
    let mut end = fit(0, limit);
    let mut overflow = None;
    if end < count {
        let digits = u16::try_from(count.to_string().len()).unwrap_or(1);
        let marker_width = 1 + digits;
        let reduced = limit.saturating_sub(marker_width + 1);
        end = fit(0, reduced);
        let active_index = view.tabs.iter().position(|tab| &tab.mode == active);
        if let Some(active_index) = active_index.filter(|index| *index >= end) {
            start = active_index;
            end = fit(start, reduced);
        }
        let hidden = count - (end - start);
        let target = if end < count { end } else { 0 };
        overflow = Some((
            Rect::new(limit.saturating_sub(marker_width), row, marker_width, 1),
            view.tabs[target].mode.clone(),
            hidden,
        ));
    }
    let mut x = left;
    let mut tabs = Vec::new();
    for (tab, width) in view.tabs[start..end].iter().zip(&widths[start..end]) {
        tabs.push((tab.mode.clone(), Rect::new(x, row, *width, 1)));
        x = x.saturating_add(*width + HEADER_TAB_GAP);
    }
    HeaderLayout {
        close,
        tabs,
        overflow,
    }
}

fn step(index: usize, len: usize, direction: RightPanelCycleDirection) -> usize {
    if len == 0 {
        return 0;
    }
    match direction {
        RightPanelCycleDirection::Next => (index + 1) % len,
        RightPanelCycleDirection::Previous => (index + len - 1) % len,
    }
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
    /// Local wall-clock time (HH:MM) the kept command exited, when known.
    pub exited_at: Option<String>,
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
    /// Last tab shown in each group view, by group label.
    pub group_tabs: std::collections::HashMap<String, RightPanelMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RightPanelState {
    pub width: PopupSize,
    pub manual_width: Option<u16>,
    pub files_command: String,
    pub open_command: String,
    pub activity_bar: RightPanelActivityBarConfig,
    pub custom_tabs: Vec<RightPanelTabConfig>,
    pub views: Vec<RightPanelView>,
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
            open_command: config.open_command,
            activity_bar: config.activity_bar,
            custom_tabs: Vec::new(),
            views: build_views(None, &[]),
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
        self.open_command = config.open_command.clone();
        self.activity_bar = config.activity_bar;
        self.custom_tabs = config.resolved_tabs().0;
        self.views = build_views(config.files_icon.as_deref(), &self.custom_tabs);
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
        self.views
            .iter()
            .flat_map(|view| view.tabs.iter().map(|tab| tab.mode.clone()))
            .collect()
    }

    pub(crate) fn view_for(&self, mode: &RightPanelMode) -> Option<&RightPanelView> {
        self.views.iter().find(|view| view.contains(mode))
    }

    pub(crate) fn header_rows(&self, mode: &RightPanelMode) -> u16 {
        self.view_for(mode).map_or(1, RightPanelView::header_rows)
    }

    pub(crate) fn is_prod(&self, mode: &RightPanelMode) -> bool {
        self.view_for(mode)
            .and_then(|view| view.tab(mode))
            .is_some_and(|tab| tab.prod)
    }

    /// Shows `mode` for the owner and remembers it as its group's last tab.
    pub(crate) fn set_mode(&mut self, owner: PaneId, mode: RightPanelMode) {
        let group = self
            .view_for(&mode)
            .filter(|view| view.group)
            .map(|view| view.label.clone());
        let pane = self.pane_mut(owner);
        if let Some(group) = group {
            pane.group_tabs.insert(group, mode.clone());
        }
        pane.mode = mode;
    }

    /// The tab a view opens on for the owner: the group's last tab, else its first.
    pub(crate) fn view_mode(&self, owner: PaneId, index: usize) -> Option<RightPanelMode> {
        let view = self.views.get(index)?;
        let remembered = self
            .pane(owner)
            .and_then(|pane| pane.group_tabs.get(&view.label))
            .filter(|mode| view.contains(mode));
        remembered
            .or_else(|| view.tabs.first().map(|tab| &tab.mode))
            .cloned()
    }

    pub(crate) fn command(&self, mode: &RightPanelMode) -> Option<&str> {
        match mode {
            RightPanelMode::Files => Some(&self.files_command),
            RightPanelMode::Custom(label) => self
                .custom_tabs
                .iter()
                .find(|tab| &tab.label == label)
                .map(|tab| tab.command.as_str()),
        }
    }

    /// The tab shown after moving one view in `direction` from the owner's current view.
    pub(crate) fn cycled(
        &self,
        owner: PaneId,
        mode: &RightPanelMode,
        direction: RightPanelCycleDirection,
    ) -> RightPanelMode {
        let len = self.views.len();
        let index = view_index(&self.views, mode).unwrap_or(0);
        let next = step(index, len, direction);
        self.view_mode(owner, next).unwrap_or(RightPanelMode::Files)
    }

    /// The neighbouring tab inside `mode`'s view, wrapping; single views stay put.
    pub(crate) fn cycled_tab(
        &self,
        mode: &RightPanelMode,
        direction: RightPanelCycleDirection,
    ) -> RightPanelMode {
        let Some(view) = self.view_for(mode) else {
            return mode.clone();
        };
        let len = view.tabs.len();
        let index = view
            .tabs
            .iter()
            .position(|tab| &tab.mode == mode)
            .unwrap_or(0);
        view.tabs[step(index, len, direction)].mode.clone()
    }

    pub(crate) fn bar_visible_for(&self, owner: Option<PaneId>) -> bool {
        match self.activity_bar {
            RightPanelActivityBarConfig::Always => true,
            RightPanelActivityBarConfig::WhenOpen => self.is_visible_for(owner),
            RightPanelActivityBarConfig::Off => false,
        }
    }

    pub(crate) fn split(&self, owner: Option<PaneId>, area: Rect) -> SurfaceSplit {
        surface_split(
            area,
            self.effective_width(),
            self.is_visible_for(owner),
            self.bar_visible_for(owner),
        )
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
        self.split(owner, area).tab
    }

    pub(crate) fn panel_rect(&self, owner: Option<PaneId>, area: Rect) -> Option<Rect> {
        self.split(owner, area).panel
    }

    pub(crate) fn bar_rect(&self, owner: Option<PaneId>, area: Rect) -> Option<Rect> {
        self.split(owner, area).bar
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
            exited_at: None,
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
    fn content_starts_below_the_header_rows() {
        let panel = Rect::new(60, 0, 40, 20);
        assert_eq!(content_rect(panel, 1), Rect::new(61, 1, 39, 19));
        assert_eq!(content_rect(panel, 2), Rect::new(61, 2, 39, 18));
    }

    fn tab(label: &str) -> RightPanelTabConfig {
        RightPanelTabConfig {
            label: label.into(),
            command: format!("run {label}"),
            ..Default::default()
        }
    }

    fn grouped(label: &str, group: &str, tag: Option<&str>) -> RightPanelTabConfig {
        RightPanelTabConfig {
            group: Some(group.into()),
            tag: tag.map(str::to_owned),
            ..tab(label)
        }
    }

    fn tabs_config(labels: &[&str]) -> RightPanelConfig {
        RightPanelConfig {
            tabs: labels.iter().map(|label| tab(label)).collect(),
            ..Default::default()
        }
    }

    fn configured(labels: &[&str]) -> RightPanelState {
        let mut state = RightPanelState::default();
        assert!(state
            .apply_config(default_width(), &tabs_config(labels))
            .is_empty());
        state
    }

    fn custom(label: &str) -> RightPanelMode {
        RightPanelMode::Custom(label.into())
    }

    #[test]
    fn modes_round_trip_through_their_labels() {
        for mode in [RightPanelMode::Files, custom("rails c"), custom("diff")] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.label()));
            assert_eq!(serde_json::from_str::<RightPanelMode>(&json).unwrap(), mode);
        }
    }

    #[test]
    fn commands_resolve_for_builtins_and_configured_tabs_only() {
        let state = configured(&["awsx"]);
        assert_eq!(state.command(&RightPanelMode::Files), Some("yazi"));
        assert_eq!(state.command(&custom("awsx")), Some("run awsx"));
        assert_eq!(state.command(&custom("gone")), None);
    }

    #[test]
    fn cycling_walks_views_and_wraps_both_ways() {
        let state = configured(&["a", "b"]);
        let owner = PaneId::alloc();
        let next =
            |mode: &RightPanelMode| state.cycled(owner, mode, RightPanelCycleDirection::Next);
        let previous =
            |mode: &RightPanelMode| state.cycled(owner, mode, RightPanelCycleDirection::Previous);

        assert_eq!(next(&RightPanelMode::Files), custom("a"));
        assert_eq!(next(&custom("a")), custom("b"));
        assert_eq!(next(&custom("b")), RightPanelMode::Files);
        assert_eq!(previous(&RightPanelMode::Files), custom("b"));
        assert_eq!(previous(&custom("a")), RightPanelMode::Files);
        assert_eq!(next(&custom("gone")), custom("a"));
    }

    fn ops_config() -> RightPanelConfig {
        RightPanelConfig {
            files_icon: Some("F".into()),
            tabs: vec![
                RightPanelTabConfig {
                    icon: Some("V".into()),
                    ..tab("vigias")
                },
                RightPanelTabConfig {
                    icon: Some("O".into()),
                    ..grouped("rails c", "ops", Some("prod"))
                },
                tab("agendados"),
                grouped("awsx", "ops", Some("staging")),
                grouped("metabase", "ops", None),
            ],
            ..Default::default()
        }
    }

    fn ops_state() -> RightPanelState {
        let mut state = RightPanelState::default();
        assert!(state
            .apply_config(default_width(), &ops_config())
            .is_empty());
        state
    }

    #[test]
    fn views_group_tabs_in_first_appearance_order_with_icons() {
        let state = ops_state();
        let summary = state
            .views
            .iter()
            .map(|view| {
                (
                    view.label.as_str(),
                    view.icon.as_str(),
                    view.group,
                    view.tabs
                        .iter()
                        .map(|tab| (tab.label.as_str(), tab.prod))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            summary,
            vec![
                ("files", "F", false, vec![("files", false)]),
                ("vigias", "V", false, vec![("vigias", false)]),
                (
                    "ops",
                    "O",
                    true,
                    vec![("rails c", true), ("awsx", false), ("metabase", false)]
                ),
                ("agendados", "A", false, vec![("agendados", false)]),
            ]
        );
        assert_eq!(
            state.modes(),
            vec![
                RightPanelMode::Files,
                custom("vigias"),
                custom("rails c"),
                custom("awsx"),
                custom("metabase"),
                custom("agendados"),
            ]
        );
        assert_eq!(build_views(None, &[])[0].icon, "F");
        assert_eq!(state.header_rows(&custom("awsx")), 2);
        assert_eq!(state.header_rows(&custom("vigias")), 1);
        assert!(state.is_prod(&custom("rails c")));
        assert!(!state.is_prod(&custom("awsx")));
    }

    #[test]
    fn views_reopen_their_last_group_tab_and_cycle_between_views() {
        let mut state = ops_state();
        let owner = PaneId::alloc();
        assert_eq!(state.view_mode(owner, 2), Some(custom("rails c")));
        assert_eq!(state.view_mode(owner, 9), None);

        state.set_mode(owner, custom("metabase"));

        assert_eq!(state.view_mode(owner, 2), Some(custom("metabase")));
        assert_eq!(
            state.cycled(owner, &custom("vigias"), RightPanelCycleDirection::Next),
            custom("metabase")
        );
        assert_eq!(
            state.cycled(owner, &custom("metabase"), RightPanelCycleDirection::Next),
            custom("agendados")
        );
        assert_eq!(
            state.cycled(owner, &custom("awsx"), RightPanelCycleDirection::Previous),
            custom("vigias")
        );
    }

    #[test]
    fn tab_cycling_stays_inside_the_group_and_wraps() {
        let state = ops_state();
        let next = |mode: &str| state.cycled_tab(&custom(mode), RightPanelCycleDirection::Next);
        let previous =
            |mode: &str| state.cycled_tab(&custom(mode), RightPanelCycleDirection::Previous);

        assert_eq!(next("rails c"), custom("awsx"));
        assert_eq!(next("metabase"), custom("rails c"));
        assert_eq!(previous("rails c"), custom("metabase"));
        assert_eq!(next("vigias"), custom("vigias"));
        assert_eq!(
            state.cycled_tab(&RightPanelMode::Files, RightPanelCycleDirection::Next),
            RightPanelMode::Files
        );
    }

    #[test]
    fn activity_bar_takes_the_right_edge_before_the_panel() {
        let area = Rect::new(0, 0, 100, 20);
        let width = PopupSize::Cells(40);

        let both = surface_split(area, width, true, true);
        assert_eq!(both.bar, Some(Rect::new(97, 0, 3, 20)));
        assert_eq!(both.panel, Some(Rect::new(57, 0, 40, 20)));
        assert_eq!(both.tab, Rect::new(0, 0, 57, 20));

        let closed = surface_split(area, width, false, true);
        assert_eq!(closed.panel, None);
        assert_eq!(closed.tab, Rect::new(0, 0, 97, 20));

        let off = surface_split(area, width, true, false);
        assert_eq!(off.bar, None);
        assert_eq!(off.panel, Some(Rect::new(60, 0, 40, 20)));

        let narrow = surface_split(Rect::new(0, 0, 22, 20), width, false, true);
        assert_eq!(narrow.bar, None);
    }

    #[test]
    fn activity_bar_visibility_follows_its_setting() {
        let owner = PaneId::alloc();
        let mut state = RightPanelState::default();
        assert!(!state.bar_visible_for(Some(owner)));
        state.activity_bar = RightPanelActivityBarConfig::Always;
        assert!(state.bar_visible_for(Some(owner)));
        assert!(state.bar_visible_for(None));
        state.activity_bar = RightPanelActivityBarConfig::WhenOpen;
        assert!(!state.bar_visible_for(Some(owner)));
        shown(&mut state, owner, RightPanelMode::Files);
        assert!(state.bar_visible_for(Some(owner)));
        assert_eq!(
            state.bar_rect(Some(owner), Rect::new(0, 0, 120, 40)),
            Some(Rect::new(117, 0, 3, 40))
        );
    }

    #[test]
    fn activity_bar_icons_leave_a_blank_row_between_views() {
        let bar = Rect::new(97, 2, 3, 6);
        assert_eq!(
            activity_bar_icons(bar, 4),
            vec![
                Rect::new(97, 3, 3, 1),
                Rect::new(97, 5, 3, 1),
                Rect::new(97, 7, 3, 1),
            ]
        );
    }

    #[test]
    fn single_view_header_has_only_the_close_button() {
        let state = ops_state();
        let panel = Rect::new(60, 0, 40, 20);
        let layout = header_layout(panel, &state.views[1], &custom("vigias"));
        assert_eq!(layout.close, Some(Rect::new(97, 0, 3, 1)));
        assert!(layout.tabs.is_empty());
        assert_eq!(layout.overflow, None);
    }

    #[test]
    fn group_header_lays_tabs_out_on_the_second_row() {
        let state = ops_state();
        let panel = Rect::new(60, 0, 60, 20);
        let layout = header_layout(panel, &state.views[2], &custom("awsx"));
        assert_eq!(
            layout.tabs,
            vec![
                (custom("rails c"), Rect::new(62, 1, 16, 1)),
                (custom("awsx"), Rect::new(80, 1, 6, 1)),
                (custom("metabase"), Rect::new(88, 1, 10, 1)),
            ]
        );
        assert_eq!(layout.overflow, None);
    }

    #[test]
    fn group_header_marks_hidden_tabs_and_keeps_the_active_one_visible() {
        let state = ops_state();
        let panel = Rect::new(0, 0, 25, 20);
        let view = &state.views[2];

        let first = header_layout(panel, view, &custom("rails c"));
        assert_eq!(
            first
                .tabs
                .iter()
                .map(|(mode, _)| mode.clone())
                .collect::<Vec<_>>(),
            vec![custom("rails c")]
        );
        assert_eq!(
            first.overflow,
            Some((Rect::new(22, 1, 2, 1), custom("awsx"), 2))
        );

        let last = header_layout(panel, view, &custom("metabase"));
        assert_eq!(
            last.tabs
                .iter()
                .map(|(mode, _)| mode.clone())
                .collect::<Vec<_>>(),
            vec![custom("metabase")]
        );
        assert_eq!(
            last.overflow.map(|(_, target, hidden)| (target, hidden)),
            Some((custom("rails c"), 2))
        );
    }

    #[test]
    fn reload_without_a_tab_falls_back_to_files_and_releases_its_instances() {
        let (a, b) = (PaneId::alloc(), PaneId::alloc());
        let mut state = configured(&["rc", "awsx"]);
        state.instances = vec![
            instance(a, custom("rc"), "/a"),
            instance(a, RightPanelMode::Files, "/a"),
            instance(b, custom("awsx"), "/b"),
        ];
        shown(&mut state, a, custom("rc"));
        shown(&mut state, b, custom("awsx"));

        let released = state.apply_config(default_width(), &tabs_config(&["awsx"]));

        assert_eq!(
            released.iter().map(|i| i.mode.clone()).collect::<Vec<_>>(),
            vec![custom("rc")]
        );
        assert_eq!(state.pane(a).unwrap().mode, RightPanelMode::Files);
        assert!(state.pane(a).unwrap().visible);
        assert_eq!(state.pane(b).unwrap().mode, custom("awsx"));
        assert_eq!(state.instances.len(), 2);
        assert_eq!(state.custom_labels(), vec!["awsx".to_owned()]);
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
        let mut state = configured(&["logs"]);
        shown(&mut state, a, RightPanelMode::Files);
        shown(&mut state, b, custom("logs"));
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
                instance(a, custom("logs"), "/a"),
                instance(b, RightPanelMode::Files, "/a"),
            ],
            ..configured(&["logs"])
        };
        assert_eq!(state.displayed(a), None);
        shown(&mut state, a, custom("logs"));

        assert_eq!(state.displayed(a), Some(&state.instances[1]));
        state.pane_mut(a).mode = RightPanelMode::Files;
        assert_eq!(state.displayed(a), Some(&state.instances[0]));
        assert_eq!(state.displayed(b), None);
        assert_eq!(state.instance_for(b, &RightPanelMode::Files), Some(2));
        assert_eq!(state.instance_for(b, &custom("logs")), None);
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
                instance(owner, custom("logs"), "/a"),
            ],
            ..configured(&["logs"])
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
                instance(owner, custom("logs"), "/a"),
            ],
            ..configured(&["logs"])
        };
        shown(&mut state, owner, custom("logs"));
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
                instance(owner, custom("logs"), "/a"),
            ],
            ..configured(&["logs"])
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
                instance(a, custom("logs"), "/a"),
            ],
            ..configured(&["logs"])
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
