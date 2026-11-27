use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use tracing::warn;

use crate::app::state::AppState;
use crate::app::App;
use crate::layout::PaneId;
use crate::pane::PaneLaunchEnv;
use crate::right_panel::{
    self, RightPanelCycleDirection, RightPanelInstance, RightPanelMode, MAX_INSTANCES,
    PANEL_OWNER_PANE_ENV, SESSION_REPOS_ENV,
};
use crate::terminal::{TerminalId, TerminalRuntime, TerminalState};
use crate::ui::TabSurfaceTarget;

impl AppState {
    pub(crate) fn right_panel_owner(&self, target: TabSurfaceTarget) -> Option<PaneId> {
        Some(
            self.workspaces
                .get(target.workspace_index)?
                .tabs
                .get(target.tab_index)?
                .layout
                .focused(),
        )
    }

    fn focused_right_panel_owner(&self) -> Option<(usize, PaneId)> {
        let ws_idx = self.active?;
        let pane_id = self.workspaces.get(ws_idx)?.focused_pane_id()?;
        Some((ws_idx, pane_id))
    }

    fn pane_exists(&self, pane_id: PaneId) -> bool {
        self.workspaces
            .iter()
            .any(|workspace| workspace.find_tab_index_for_pane(pane_id).is_some())
    }
}

impl App {
    pub(crate) fn toggle_right_panel(&mut self) {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return;
        };
        self.release_exited_right_panel_instances(owner);
        let pane = self.state.right_panel.pane_mut(owner);
        pane.visible = !pane.visible;
        pane.focused = pane.visible;
        if !pane.visible {
            pane.pending_open = None;
        }
        self.request_right_panel_render();
    }

    pub(crate) fn show_right_panel(&mut self, mode: RightPanelMode) {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return;
        };
        self.release_exited_right_panel_instances(owner);
        self.state.right_panel.set_mode(owner, mode);
        let pane = self.state.right_panel.pane_mut(owner);
        pane.visible = true;
        pane.focused = true;
        self.request_right_panel_render();
    }

    /// Opens a view on its last tab, or closes the panel when that view is already shown.
    pub(crate) fn activate_right_panel_view(&mut self, index: usize) -> bool {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return false;
        };
        let Some(mode) = self.state.right_panel.view_mode(owner, index) else {
            return false;
        };
        let shown = self.state.right_panel.pane(owner).is_some_and(|pane| {
            pane.visible
                && right_panel::view_index(&self.state.right_panel.views, &pane.mode) == Some(index)
        });
        if shown {
            self.hide_right_panel();
        } else {
            self.show_right_panel(mode);
        }
        true
    }

    pub(crate) fn cycle_right_panel_tab(&mut self, direction: RightPanelCycleDirection) {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return;
        };
        self.release_exited_right_panel_instances(owner);
        let current = self.state.right_panel.pane_mut(owner).mode.clone();
        let mode = self.state.right_panel.cycled_tab(&current, direction);
        self.state.right_panel.set_mode(owner, mode);
        let pane = self.state.right_panel.pane_mut(owner);
        if !pane.visible {
            pane.visible = true;
            pane.focused = true;
        }
        self.request_right_panel_render();
    }

    pub(crate) fn hide_right_panel(&mut self) {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return;
        };
        self.release_exited_right_panel_instances(owner);
        let pane = self.state.right_panel.pane_mut(owner);
        pane.visible = false;
        pane.focused = false;
        pane.pending_open = None;
        self.request_right_panel_render();
    }

    /// Starts the shown tab again: drops its instance (running or exited) so the next sync spawns a fresh one.
    /// On files this is also the ‹ back button: the opened file closes and the explorer starts again.
    pub(crate) fn restart_right_panel(&mut self) {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return;
        };
        self.release_exited_right_panel_instances(owner);
        let mode = self.state.right_panel.pane_mut(owner).mode.clone();
        if let Some(instance) = self.state.right_panel.take_instance(owner, &mode) {
            self.release_right_panel_instance(instance);
        }
        let pane = self.state.right_panel.pane_mut(owner);
        pane.visible = true;
        pane.focused = true;
        self.request_right_panel_render();
    }

    pub(crate) fn cycle_right_panel(&mut self, direction: RightPanelCycleDirection) {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return;
        };
        self.release_exited_right_panel_instances(owner);
        let current = self.state.right_panel.pane_mut(owner).mode.clone();
        let mode = self.state.right_panel.cycled(owner, &current, direction);
        self.state.right_panel.set_mode(owner, mode);
        let pane = self.state.right_panel.pane_mut(owner);
        if !pane.visible {
            pane.visible = true;
            pane.focused = true;
        }
        self.request_right_panel_render();
    }

    pub(crate) fn open_right_panel(
        &mut self,
        target: &crate::right_panel::OpenTarget,
        owner: Option<PaneId>,
    ) -> bool {
        let focused = self.state.focused_right_panel_owner().map(|(_, pane)| pane);
        let Some(owner) = owner.or(focused) else {
            return false;
        };
        let command = match target {
            crate::right_panel::OpenTarget::File { path, line } => {
                Some(crate::right_panel::render_open_command(
                    &self.state.right_panel.open_command,
                    path,
                    *line,
                ))
            }
            crate::right_panel::OpenTarget::Dir(_) => None,
        };
        self.release_exited_right_panel_instances(owner);
        let pane = self.state.right_panel.pane_mut(owner);
        pane.pending_open = Some(crate::right_panel::PendingOpen {
            dir: target.dir(),
            command,
        });
        pane.mode = RightPanelMode::Files;
        pane.visible = true;
        if focused == Some(owner) {
            pane.focused = true;
        }
        self.request_right_panel_render();
        true
    }

    pub(crate) fn apply_right_panel_config(
        &mut self,
        width: crate::popup_size::PopupSize,
        config: &crate::config::RightPanelConfig,
    ) {
        for removed in self.state.right_panel.apply_config(width, config) {
            self.release_right_panel_instance(removed);
        }
        self.request_right_panel_render();
    }

    pub(crate) fn set_right_panel_width(&mut self, width: Option<u16>) {
        if self.state.right_panel.manual_width != width {
            self.state.right_panel.manual_width = width;
            self.request_right_panel_render();
        }
    }

    fn release_exited_right_panel_instances(&mut self, owner: PaneId) {
        for instance in self.state.right_panel.take_exited_for(owner) {
            self.release_right_panel_instance(instance);
        }
    }

    pub(crate) fn focus_right_panel(&mut self, terminal_id: &str) -> bool {
        let Some(owner) = self
            .state
            .right_panel
            .owns_terminal(terminal_id)
            .map(|instance| instance.owner)
        else {
            return false;
        };
        let shown = self
            .state
            .right_panel
            .displayed(owner)
            .is_some_and(|instance| instance.terminal_id.as_str() == terminal_id);
        let focused_owner = self.state.focused_right_panel_owner().map(|(_, pane)| pane);
        if !shown || focused_owner != Some(owner) {
            return false;
        }
        let pane = self.state.right_panel.pane_mut(owner);
        if !pane.focused {
            pane.focused = true;
            self.request_right_panel_render();
        }
        true
    }

    pub(crate) fn blur_right_panel(&mut self) {
        let Some((_, owner)) = self.state.focused_right_panel_owner() else {
            return;
        };
        let Some(pane) = self.state.right_panel.panes.get_mut(&owner) else {
            return;
        };
        if std::mem::take(&mut pane.focused) {
            self.request_right_panel_render();
        }
    }

    pub(crate) fn right_panel_runtime(&self, public_id: &str) -> Option<&TerminalRuntime> {
        let terminal_id = right_panel::parse_public_id(public_id)?;
        let instance = self.state.right_panel.owns_terminal(terminal_id)?;
        self.terminal_runtimes.get(&instance.terminal_id)
    }

    pub(crate) fn focused_right_panel_runtime(
        &self,
        owner: PaneId,
    ) -> Option<(PaneId, &TerminalRuntime)> {
        let state = &self.state.right_panel;
        state.pane(owner).filter(|pane| pane.focused)?;
        let instance = state.displayed(owner)?;
        let runtime = self.terminal_runtimes.get(&instance.terminal_id)?;
        Some((instance.pane_id, runtime))
    }

    pub(crate) fn right_panel_pane_runtime(
        &self,
        public_id: &str,
    ) -> Option<(PaneId, &TerminalRuntime)> {
        let terminal_id = right_panel::parse_public_id(public_id)?;
        let instance = self.state.right_panel.owns_terminal(terminal_id)?;
        let runtime = self.terminal_runtimes.get(&instance.terminal_id)?;
        Some((instance.pane_id, runtime))
    }

    pub(crate) fn right_panel_pane_died(&mut self, pane_id: PaneId) -> bool {
        let Some(exit) = self
            .state
            .right_panel
            .command_exited(pane_id, std::time::Instant::now())
        else {
            return false;
        };
        match exit {
            crate::right_panel::PanelExit::Released(instance) => {
                self.release_right_panel_instance(instance);
            }
            crate::right_panel::PanelExit::KeptFailedStart => {
                let exited_at = crate::platform::local_datetime()
                    .map(|time| format!("{:02}:{:02}", time.hour(), time.minute()));
                if let Some(instance) = self
                    .state
                    .right_panel
                    .instances
                    .iter_mut()
                    .find(|instance| instance.pane_id == pane_id)
                {
                    instance.exited_at = exited_at;
                }
            }
        }
        self.request_right_panel_render();
        true
    }

    pub(crate) fn release_right_panel_owner(&mut self, owner: PaneId) {
        let removed = self.state.right_panel.remove_owner(owner);
        let changed = !removed.is_empty();
        for instance in removed {
            self.release_right_panel_instance(instance);
        }
        if changed {
            self.request_right_panel_render();
        }
    }

    fn prune_right_panel_owners(&mut self) {
        let gone = self
            .state
            .right_panel
            .panes
            .keys()
            .copied()
            .chain(self.state.right_panel.owning_panes())
            .filter(|owner| !self.state.pane_exists(*owner))
            .collect::<std::collections::HashSet<_>>();
        for owner in gone {
            self.release_right_panel_owner(owner);
        }
    }

    pub(crate) fn sync_right_panel(&mut self, target: TabSurfaceTarget, panel: Rect) {
        self.prune_right_panel_owners();
        let Some(owner) = self.state.right_panel_owner(target) else {
            return;
        };
        let Some(pane) = self
            .state
            .right_panel
            .pane(owner)
            .filter(|pane| pane.visible)
            .cloned()
        else {
            return;
        };
        if let Some(open) = self.state.right_panel.pane_mut(owner).pending_open.take() {
            self.apply_right_panel_open(target, owner, open, panel);
            return;
        }
        if let Some(index) = self.state.right_panel.instance_for(owner, &pane.mode) {
            self.state.right_panel.touch(index);
            return;
        }
        let Some(command) = self
            .state
            .right_panel
            .command(&pane.mode)
            .map(str::to_owned)
        else {
            self.state.right_panel.pane_mut(owner).mode = RightPanelMode::Files;
            self.sync_right_panel(target, panel);
            return;
        };
        let dir = self.right_panel_dir(Some((target.workspace_index, owner)));
        self.start_right_panel_instance(owner, pane.mode, dir, command, panel);
    }

    fn apply_right_panel_open(
        &mut self,
        target: TabSurfaceTarget,
        owner: PaneId,
        open: right_panel::PendingOpen,
        panel: Rect,
    ) {
        let reusable = open.command.is_none()
            && self
                .state
                .right_panel
                .instance_for(owner, &RightPanelMode::Files)
                .is_some_and(|index| {
                    let instance = &self.state.right_panel.instances[index];
                    !instance.exited && instance.dir == open.dir
                });
        if reusable {
            self.sync_right_panel(target, panel);
            return;
        }
        if let Some(replaced) = self
            .state
            .right_panel
            .take_instance(owner, &RightPanelMode::Files)
        {
            self.release_right_panel_instance(replaced);
        }
        let command = open
            .command
            .unwrap_or_else(|| self.state.right_panel.files_command.clone());
        self.start_right_panel_instance(owner, RightPanelMode::Files, open.dir, command, panel);
    }

    fn start_right_panel_instance(
        &mut self,
        owner: PaneId,
        mode: RightPanelMode,
        dir: PathBuf,
        command: String,
        panel: Rect,
    ) {
        let content = right_panel::content_rect(panel, self.state.right_panel.header_rows(&mode));
        match self.spawn_right_panel_instance(
            owner,
            mode,
            dir,
            &command,
            content.height,
            content.width,
        ) {
            Ok(instance) => {
                self.state.right_panel.instances.push(instance);
                let last = self.state.right_panel.instances.len() - 1;
                let shown = self.state.right_panel.touch(last);
                for evicted in self.state.right_panel.evict_over(MAX_INSTANCES, &[shown]) {
                    self.release_right_panel_instance(evicted);
                }
            }
            Err(err) => {
                warn!(err = %err, "right panel command failed to start");
                let pane = self.state.right_panel.pane_mut(owner);
                pane.visible = false;
                pane.focused = false;
            }
        }
    }

    fn right_panel_dir(&self, target: Option<(usize, PaneId)>) -> PathBuf {
        let worktree =
            target.and_then(|(ws_idx, pane_id)| self.pane_claude_worktree(ws_idx, pane_id));
        let pane_cwd = target.and_then(|(ws_idx, pane_id)| {
            let workspace = self.state.workspaces.get(ws_idx)?;
            let tab = workspace
                .tabs
                .get(workspace.find_tab_index_for_pane(pane_id)?)?;
            tab.foreground_cwd_for_pane(pane_id, &self.terminal_runtimes)
                .or_else(|| {
                    tab.cwd_for_pane(pane_id, &self.state.terminals, &self.terminal_runtimes)
                })
        });
        right_panel::resolve_dir(
            worktree.as_deref().map(Path::new),
            pane_cwd,
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }

    fn pane_claude_worktree(&self, ws_idx: usize, pane_id: PaneId) -> Option<String> {
        self.pane_claude_session(ws_idx, pane_id)?.worktree_path
    }

    fn pane_claude_session(
        &self,
        ws_idx: usize,
        pane_id: PaneId,
    ) -> Option<crate::claude_sessions::ClaudeSession> {
        let terminal_id = self.state.workspaces.get(ws_idx)?.terminal_id(pane_id)?;
        let terminal = self.state.terminals.get(terminal_id)?;
        let session = crate::app::creation::terminal_agent_session_info(terminal)?;
        if session.agent != "claude" {
            return None;
        }
        self.claude_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|candidate| candidate.session_id == session.value)
            .cloned()
    }

    fn right_panel_env(&self, owner: PaneId) -> Vec<(String, String)> {
        let mut env = self.custom_command_env().0;
        let located = self.find_pane(owner).map(|(ws_idx, _)| ws_idx);
        if let Some(owner_id) = located.and_then(|ws_idx| self.public_pane_id(ws_idx, owner)) {
            env.push((PANEL_OWNER_PANE_ENV.to_owned(), owner_id));
        }
        let repos = located
            .and_then(|ws_idx| self.pane_claude_session(ws_idx, owner))
            .map(|session| session.repos.join("\n"))
            .unwrap_or_default();
        env.push((SESSION_REPOS_ENV.to_owned(), repos));
        env
    }

    fn spawn_right_panel_instance(
        &mut self,
        owner: PaneId,
        mode: RightPanelMode,
        dir: PathBuf,
        command: &str,
        rows: u16,
        cols: u16,
    ) -> std::io::Result<RightPanelInstance> {
        let pane_id = PaneId::alloc();
        let terminal_id = TerminalId::alloc();
        let launch_env =
            PaneLaunchEnv::from_extra(self.right_panel_env(owner)).without_pane_identity();
        let runtime = TerminalRuntime::spawn_shell_command(
            pane_id,
            rows.max(1),
            cols.max(1),
            dir.clone(),
            command,
            &launch_env,
            crate::pane::AgentDetection::Disabled,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.state.host_terminal_appearance,
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        )?;
        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        self.state.terminals.insert(
            terminal_id.clone(),
            TerminalState::new(terminal_id.clone(), dir.clone()),
        );
        Ok(RightPanelInstance {
            owner,
            mode,
            dir,
            pane_id,
            terminal_id,
            spawned_at: std::time::Instant::now(),
            exited: false,
            exited_at: None,
        })
    }

    fn release_right_panel_instance(&mut self, instance: RightPanelInstance) {
        self.state
            .direct_attach_resize_locks
            .remove(&instance.terminal_id);
        self.state.terminals.remove(&instance.terminal_id);
        self.shutdown_terminal_runtime(instance.terminal_id);
    }

    fn request_right_panel_render(&self) {
        self.render_dirty.request_generic();
        self.render_notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![crate::workspace::Workspace::test_new("panel")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app
    }

    fn target() -> TabSurfaceTarget {
        TabSurfaceTarget {
            workspace_index: 0,
            tab_index: 0,
        }
    }

    fn focused(app: &App) -> PaneId {
        app.state.workspaces[0].focused_pane_id().unwrap()
    }

    fn split(app: &mut App) -> (PaneId, PaneId) {
        let first = focused(app);
        let second = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
        app.state.focus_pane_in_workspace(0, first);
        (first, second)
    }

    fn install_instance(
        app: &mut App,
        owner: PaneId,
        mode: RightPanelMode,
        dir: &str,
    ) -> RightPanelInstance {
        let instance = RightPanelInstance {
            owner,
            mode,
            dir: PathBuf::from(dir),
            pane_id: PaneId::alloc(),
            terminal_id: TerminalId::alloc(),
            spawned_at: std::time::Instant::now(),
            exited: false,
            exited_at: None,
        };
        app.state.terminals.insert(
            instance.terminal_id.clone(),
            TerminalState::new(instance.terminal_id.clone(), PathBuf::from(dir)),
        );
        app.state.right_panel.instances.push(instance.clone());
        instance
    }

    fn displayed(app: &App, owner: PaneId) -> Option<TerminalId> {
        app.state
            .right_panel
            .displayed(owner)
            .map(|instance| instance.terminal_id.clone())
    }

    const PANEL: Rect = Rect::new(60, 0, 40, 20);

    #[test]
    fn toggle_shows_focused_then_hides_unfocused_for_the_focused_pane() {
        let mut app = test_app();
        let owner = focused(&app);

        app.toggle_right_panel();
        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible && pane.focused);

        app.toggle_right_panel();
        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(!pane.visible && !pane.focused);
    }

    #[test]
    fn panel_state_is_isolated_per_pane_and_restored_on_return() {
        let mut app = test_app();
        let (a, b) = split(&mut app);
        let a_files = install_instance(&mut app, a, RightPanelMode::Files, "/a");
        app.toggle_right_panel();
        app.sync_right_panel(target(), PANEL);
        assert_eq!(displayed(&app, a), Some(a_files.terminal_id.clone()));

        app.state.focus_pane_in_workspace(0, b);
        assert!(!app.state.right_panel.is_visible_for(Some(b)));
        assert_eq!(app.state.right_panel_owner(target()), Some(b));
        app.sync_right_panel(target(), PANEL);
        assert_eq!(app.state.right_panel.pane(b), None);

        app.state.focus_pane_in_workspace(0, a);
        app.sync_right_panel(target(), PANEL);
        assert_eq!(displayed(&app, a), Some(a_files.terminal_id));
        assert!(app.state.right_panel.is_visible_for(Some(a)));
    }

    #[test]
    fn mode_is_per_pane() {
        let mut app = test_app();
        let (a, b) = split(&mut app);
        configure_tabs(&mut app, &["logs"]);
        app.show_right_panel(custom("logs"));
        app.state.focus_pane_in_workspace(0, b);
        app.show_right_panel(RightPanelMode::Files);

        assert_eq!(app.state.right_panel.pane(a).unwrap().mode, custom("logs"));
        assert_eq!(
            app.state.right_panel.pane(b).unwrap().mode,
            RightPanelMode::Files
        );
    }

    #[test]
    fn cycle_next_walks_the_header_order_and_wraps() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs", "jobs"]);
        app.show_right_panel(RightPanelMode::Files);

        let mut seen = Vec::new();
        for _ in 0..3 {
            app.cycle_right_panel(RightPanelCycleDirection::Next);
            seen.push(app.state.right_panel.pane(owner).unwrap().mode.clone());
        }

        assert_eq!(
            seen,
            vec![custom("logs"), custom("jobs"), RightPanelMode::Files]
        );
    }

    #[test]
    fn cycle_previous_walks_backwards_and_wraps() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs", "jobs"]);
        app.show_right_panel(RightPanelMode::Files);

        let mut seen = Vec::new();
        for _ in 0..3 {
            app.cycle_right_panel(RightPanelCycleDirection::Previous);
            seen.push(app.state.right_panel.pane(owner).unwrap().mode.clone());
        }

        assert_eq!(
            seen,
            vec![custom("jobs"), custom("logs"), RightPanelMode::Files]
        );
    }

    fn configure_tabs(app: &mut App, labels: &[&str]) {
        let config = crate::config::RightPanelConfig {
            tabs: labels
                .iter()
                .map(|label| crate::config::RightPanelTabConfig {
                    label: (*label).into(),
                    command: "sleep 30".into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let width = app.state.right_panel.width;
        app.apply_right_panel_config(width, &config);
    }

    fn custom(label: &str) -> RightPanelMode {
        RightPanelMode::Custom(label.into())
    }

    fn configure_ops(app: &mut App) {
        let tab = |label: &str, group: Option<&str>| crate::config::RightPanelTabConfig {
            label: label.into(),
            command: "sleep 30".into(),
            group: group.map(str::to_owned),
            ..Default::default()
        };
        let config = crate::config::RightPanelConfig {
            tabs: vec![
                tab("vigias", None),
                tab("rails c", Some("ops")),
                tab("awsx", Some("ops")),
            ],
            ..Default::default()
        };
        let width = app.state.right_panel.width;
        app.apply_right_panel_config(width, &config);
    }

    #[test]
    fn activating_a_view_opens_it_and_activating_it_again_closes_the_panel() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_ops(&mut app);

        assert!(app.activate_right_panel_view(1));
        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible && pane.focused);
        assert_eq!(pane.mode, custom("vigias"));

        assert!(app.activate_right_panel_view(2));
        assert_eq!(
            app.state.right_panel.pane(owner).unwrap().mode,
            custom("rails c")
        );

        assert!(app.activate_right_panel_view(2));
        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(!pane.visible && !pane.focused);
        assert!(!app.activate_right_panel_view(7));
    }

    #[test]
    fn a_group_view_reopens_on_its_last_tab_and_tabs_cycle_inside_it() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_ops(&mut app);
        app.activate_right_panel_view(2);

        app.cycle_right_panel_tab(RightPanelCycleDirection::Next);
        assert_eq!(
            app.state.right_panel.pane(owner).unwrap().mode,
            custom("awsx")
        );
        app.cycle_right_panel_tab(RightPanelCycleDirection::Next);
        assert_eq!(
            app.state.right_panel.pane(owner).unwrap().mode,
            custom("rails c")
        );
        app.cycle_right_panel_tab(RightPanelCycleDirection::Previous);

        app.activate_right_panel_view(0);
        app.cycle_right_panel(RightPanelCycleDirection::Next);
        app.cycle_right_panel(RightPanelCycleDirection::Next);
        assert_eq!(
            app.state.right_panel.pane(owner).unwrap().mode,
            custom("awsx")
        );
        app.cycle_right_panel_tab(RightPanelCycleDirection::Next);
        app.activate_right_panel_view(1);
        app.activate_right_panel_view(2);
        assert_eq!(
            app.state.right_panel.pane(owner).unwrap().mode,
            custom("rails c")
        );
    }

    #[test]
    fn hide_releases_an_exited_tab_and_restart_shows_it_again() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs"]);
        let logs = install_instance(&mut app, owner, custom("logs"), "/a");
        app.show_right_panel(custom("logs"));
        app.right_panel_pane_died(logs.pane_id);
        assert!(app.state.right_panel.displayed(owner).unwrap().exited);

        app.restart_right_panel();

        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible && pane.focused);
        assert_eq!(pane.mode, custom("logs"));
        assert!(app.state.right_panel.instances.is_empty());
        assert!(!app.state.terminals.contains_key(&logs.terminal_id));

        let again = install_instance(&mut app, owner, custom("logs"), "/a");
        app.right_panel_pane_died(again.pane_id);
        app.hide_right_panel();

        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(!pane.visible && !pane.focused);
        assert!(app.state.right_panel.instances.is_empty());
    }

    #[test]
    fn restart_drops_a_running_tab_and_keeps_the_others() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs", "jobs"]);
        let jobs = install_instance(&mut app, owner, custom("jobs"), "/a");
        let logs = install_instance(&mut app, owner, custom("logs"), "/a");
        app.show_right_panel(custom("logs"));

        app.restart_right_panel();

        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible && pane.focused);
        assert_eq!(pane.mode, custom("logs"));
        assert!(!app.state.terminals.contains_key(&logs.terminal_id));
        assert!(app.state.terminals.contains_key(&jobs.terminal_id));
        assert_eq!(app.state.right_panel.instances.len(), 1);
    }

    #[test]
    fn an_early_exit_records_the_local_time_for_the_card() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs"]);
        let logs = install_instance(&mut app, owner, custom("logs"), "/a");
        app.show_right_panel(custom("logs"));

        app.right_panel_pane_died(logs.pane_id);

        let exited_at = app
            .state
            .right_panel
            .displayed(owner)
            .unwrap()
            .exited_at
            .clone();
        if crate::platform::local_datetime().is_some() {
            let exited_at = exited_at.expect("exit time");
            assert_eq!(exited_at.len(), 5);
            assert_eq!(&exited_at[2..3], ":");
        }
    }

    #[tokio::test]
    async fn tabs_start_only_when_first_shown_and_stay_alive_across_views() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_ops(&mut app);
        let files = install_instance(&mut app, owner, RightPanelMode::Files, "/a");

        app.activate_right_panel_view(2);
        app.sync_right_panel(target(), PANEL);
        let started = |app: &App| {
            app.state
                .right_panel
                .instances
                .iter()
                .map(|instance| instance.mode.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            started(&app),
            vec![RightPanelMode::Files, custom("rails c")]
        );
        let rails = app.state.right_panel.displayed(owner).cloned().unwrap();
        let header_rows = app.state.right_panel.header_rows(&rails.mode);
        assert_eq!(
            app.terminal_runtimes
                .get(&rails.terminal_id)
                .unwrap()
                .current_size(),
            (PANEL.height - header_rows, PANEL.width - 1)
        );

        app.activate_right_panel_view(0);
        app.sync_right_panel(target(), PANEL);

        assert_eq!(displayed(&app, owner), Some(files.terminal_id));
        assert!(app.state.terminals.contains_key(&rails.terminal_id));
        assert_eq!(started(&app).len(), 2);
        app.release_right_panel_instance(rails);
    }

    #[test]
    fn cycle_reaches_custom_tabs_and_wraps_to_files() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["rails c", "awsx"]);
        app.show_right_panel(RightPanelMode::Files);

        let mut seen = Vec::new();
        for _ in 0..3 {
            app.cycle_right_panel(RightPanelCycleDirection::Next);
            seen.push(app.state.right_panel.pane(owner).unwrap().mode.clone());
        }
        app.cycle_right_panel(RightPanelCycleDirection::Previous);
        seen.push(app.state.right_panel.pane(owner).unwrap().mode.clone());

        assert_eq!(
            seen,
            vec![
                custom("rails c"),
                custom("awsx"),
                RightPanelMode::Files,
                custom("awsx"),
            ]
        );
    }

    #[test]
    fn switching_away_from_a_custom_tab_keeps_its_terminal() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["rc"]);
        let files = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        let rc = install_instance(&mut app, owner, custom("rc"), "/a");

        app.show_right_panel(custom("rc"));
        app.sync_right_panel(target(), PANEL);
        assert_eq!(displayed(&app, owner), Some(rc.terminal_id.clone()));
        app.show_right_panel(RightPanelMode::Files);
        app.sync_right_panel(target(), PANEL);
        assert_eq!(displayed(&app, owner), Some(files.terminal_id));
        app.show_right_panel(custom("rc"));
        app.sync_right_panel(target(), PANEL);

        assert_eq!(displayed(&app, owner), Some(rc.terminal_id.clone()));
        assert!(app.state.terminals.contains_key(&rc.terminal_id));
    }

    #[test]
    fn reload_removing_the_visible_custom_tab_releases_it_and_shows_files() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["rc", "awsx"]);
        let files = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        let rc = install_instance(&mut app, owner, custom("rc"), "/a");
        app.show_right_panel(custom("rc"));

        configure_tabs(&mut app, &["awsx"]);
        app.sync_right_panel(target(), PANEL);

        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible);
        assert_eq!(pane.mode, RightPanelMode::Files);
        assert!(!app.state.terminals.contains_key(&rc.terminal_id));
        assert_eq!(displayed(&app, owner), Some(files.terminal_id));
    }

    #[test]
    fn a_mode_without_a_command_falls_back_to_files() {
        let mut app = test_app();
        let owner = focused(&app);
        let files = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        app.show_right_panel(custom("gone"));

        app.sync_right_panel(target(), PANEL);

        assert_eq!(
            app.state.right_panel.pane(owner).unwrap().mode,
            RightPanelMode::Files
        );
        assert_eq!(displayed(&app, owner), Some(files.terminal_id));
    }

    #[test]
    fn panel_env_lists_the_owner_claude_session_repos() {
        let mut app = test_app();
        app.state.ensure_test_terminals();
        let owner = focused(&app);
        let session_id = "0da32074-acd6-4c79-9d29-aac5cc63ca81";
        let repos_env = |app: &App| {
            app.right_panel_env(owner)
                .into_iter()
                .filter(|(key, _)| key == SESSION_REPOS_ENV)
                .map(|(_, value)| value)
                .collect::<Vec<_>>()
        };
        assert_eq!(repos_env(&app), vec![String::new()]);

        let terminal_id = app.state.workspaces[0].terminal_id(owner).unwrap().clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
                source: "herdr:claude".into(),
                agent: "claude".into(),
                session_ref: crate::agent_resume::AgentSessionRef::id(session_id).unwrap(),
            });
        app.claude_sessions
            .lock()
            .unwrap()
            .push(crate::claude_sessions::ClaudeSession {
                session_id: session_id.into(),
                title: String::new(),
                cwd: "/r/api".into(),
                context: String::new(),
                worktree_path: None,
                repos: vec!["/r/web".into(), "/r/api".into()],
                updated_at_ms: 0,
            });

        assert_eq!(repos_env(&app), vec!["/r/web\n/r/api".to_owned()]);
        let env = app.right_panel_env(owner);
        assert!(env.iter().any(|(key, _)| key == PANEL_OWNER_PANE_ENV));
    }

    #[test]
    fn cycle_opens_a_hidden_panel_focused_at_the_resulting_mode() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs"]);

        app.cycle_right_panel(RightPanelCycleDirection::Next);

        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible && pane.focused);
        assert_eq!(pane.mode, custom("logs"));
    }

    #[test]
    fn cycle_on_a_visible_panel_keeps_keyboard_focus_where_it_is() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs"]);
        app.show_right_panel(RightPanelMode::Files);
        app.blur_right_panel();

        app.cycle_right_panel(RightPanelCycleDirection::Next);

        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible && !pane.focused);
        assert_eq!(pane.mode, custom("logs"));
    }

    #[test]
    fn cycle_only_changes_the_focused_pane_mode() {
        let mut app = test_app();
        let (a, b) = split(&mut app);
        configure_tabs(&mut app, &["logs", "jobs"]);
        app.show_right_panel(custom("logs"));
        app.state.focus_pane_in_workspace(0, b);

        app.cycle_right_panel(RightPanelCycleDirection::Previous);

        assert_eq!(app.state.right_panel.pane(a).unwrap().mode, custom("logs"));
        assert_eq!(app.state.right_panel.pane(b).unwrap().mode, custom("jobs"));
    }

    #[test]
    fn instances_are_not_shared_between_panes_in_the_same_directory() {
        let mut app = test_app();
        let (a, b) = split(&mut app);
        let a_files = install_instance(&mut app, a, RightPanelMode::Files, "/same");
        let b_files = install_instance(&mut app, b, RightPanelMode::Files, "/same");
        app.toggle_right_panel();
        app.state.focus_pane_in_workspace(0, b);
        app.toggle_right_panel();

        assert_eq!(displayed(&app, a), Some(a_files.terminal_id));
        assert_eq!(displayed(&app, b), Some(b_files.terminal_id));
    }

    #[test]
    fn open_for_an_unfocused_pane_prepares_it_without_stealing_the_view() {
        let mut app = test_app();
        let (a, b) = split(&mut app);

        assert!(app.open_right_panel(&crate::right_panel::OpenTarget::Dir("/b".into()), Some(b)));

        assert_eq!(focused(&app), a);
        assert!(!app.state.right_panel.is_visible_for(Some(a)));
        let pane = app.state.right_panel.pane(b).unwrap();
        assert!(pane.visible && !pane.focused);
        assert_eq!(pane.mode, RightPanelMode::Files);
        assert!(pane.pending_open.is_some());
    }

    #[test]
    fn open_without_owner_targets_and_focuses_the_focused_pane() {
        let mut app = test_app();
        let owner = focused(&app);

        assert!(app.open_right_panel(&crate::right_panel::OpenTarget::Dir("/x".into()), None));

        let pane = app.state.right_panel.pane(owner).unwrap();
        assert!(pane.visible && pane.focused);
    }

    #[test]
    fn open_directory_reuses_the_pane_files_instance_in_that_directory() {
        let mut app = test_app();
        let owner = focused(&app);
        let files = install_instance(&mut app, owner, RightPanelMode::Files, "/d");

        app.open_right_panel(&crate::right_panel::OpenTarget::Dir("/d".into()), None);
        app.sync_right_panel(target(), PANEL);

        assert_eq!(displayed(&app, owner), Some(files.terminal_id));
        assert_eq!(
            app.state.right_panel.pane(owner).unwrap().pending_open,
            None
        );
    }

    #[tokio::test]
    async fn open_file_replaces_only_that_pane_files_instance() {
        let mut app = test_app();
        app.state.right_panel.open_command = "sleep 30".into();
        let (a, b) = split(&mut app);
        let dir = std::env::temp_dir();
        let file = dir.join(format!("herdr-open-{}.txt", std::process::id()));
        std::fs::write(&file, "x\n").unwrap();
        let a_old = install_instance(&mut app, a, RightPanelMode::Files, dir.to_str().unwrap());
        let b_files = install_instance(&mut app, b, RightPanelMode::Files, dir.to_str().unwrap());

        app.open_right_panel(
            &crate::right_panel::OpenTarget::File {
                path: file.clone(),
                line: 3,
            },
            Some(a),
        );
        app.sync_right_panel(target(), PANEL);

        let shown = app.state.right_panel.displayed(a).cloned().unwrap();
        assert_ne!(shown.terminal_id, a_old.terminal_id);
        assert_eq!(shown.dir, dir);
        assert!(!app.state.terminals.contains_key(&a_old.terminal_id));
        assert!(app
            .state
            .right_panel
            .owns_terminal(b_files.terminal_id.as_str())
            .is_some());
        let _ = std::fs::remove_file(&file);
        app.release_right_panel_instance(shown);
    }

    #[test]
    fn focus_moves_between_panel_and_panes() {
        let mut app = test_app();
        let owner = focused(&app);
        let instance = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        app.state.right_panel.pane_mut(owner).visible = true;

        assert!(!app.focus_right_panel("term_other"));
        assert!(app.focus_right_panel(instance.terminal_id.as_str()));
        assert!(app.state.right_panel.pane(owner).unwrap().focused);

        app.blur_right_panel();
        assert!(!app.state.right_panel.pane(owner).unwrap().focused);
    }

    #[test]
    fn focusing_another_pane_panel_is_refused() {
        let mut app = test_app();
        let (_, b) = split(&mut app);
        let b_files = install_instance(&mut app, b, RightPanelMode::Files, "/b");
        app.state.right_panel.pane_mut(b).visible = true;

        assert!(!app.focus_right_panel(b_files.terminal_id.as_str()));
    }

    #[test]
    fn closing_a_tiled_pane_releases_its_instances() {
        let mut app = test_app();
        let (a, b) = split(&mut app);
        let a_files = install_instance(&mut app, a, RightPanelMode::Files, "/a");
        let b_files = install_instance(&mut app, b, RightPanelMode::Files, "/b");
        app.state.right_panel.pane_mut(b).visible = true;

        app.release_right_panel_owner(b);

        assert!(app.state.right_panel.pane(b).is_none());
        assert!(!app.state.terminals.contains_key(&b_files.terminal_id));
        assert!(app.state.terminals.contains_key(&a_files.terminal_id));
    }

    #[test]
    fn sync_prunes_state_for_panes_that_no_longer_exist() {
        let mut app = test_app();
        let gone = PaneId::alloc();
        let orphan = install_instance(&mut app, gone, RightPanelMode::Files, "/gone");
        app.state.right_panel.pane_mut(gone).visible = true;

        app.sync_right_panel(target(), PANEL);

        assert!(app.state.right_panel.pane(gone).is_none());
        assert!(!app.state.terminals.contains_key(&orphan.terminal_id));
    }

    #[test]
    fn hide_show_hide_keeps_the_pane_instance_for_reuse() {
        let mut app = test_app();
        let owner = focused(&app);
        let instance = install_instance(&mut app, owner, RightPanelMode::Files, "/a");

        app.toggle_right_panel();
        app.toggle_right_panel();
        app.toggle_right_panel();
        app.sync_right_panel(target(), PANEL);
        assert_eq!(displayed(&app, owner), Some(instance.terminal_id.clone()));
        app.toggle_right_panel();

        assert!(!app.state.right_panel.is_visible_for(Some(owner)));
        assert_eq!(app.state.right_panel.instances, vec![instance.clone()]);
        assert!(app.state.terminals.contains_key(&instance.terminal_id));
    }

    #[test]
    fn failed_start_stays_visible_and_the_next_show_respawns() {
        let mut app = test_app();
        let owner = focused(&app);
        configure_tabs(&mut app, &["logs"]);
        let files = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        let logs = install_instance(&mut app, owner, custom("logs"), "/a");
        app.show_right_panel(custom("logs"));

        assert!(app.right_panel_pane_died(logs.pane_id));
        assert!(app.state.right_panel.is_visible_for(Some(owner)));
        assert!(app.state.terminals.contains_key(&logs.terminal_id));

        app.show_right_panel(RightPanelMode::Files);

        assert_eq!(app.state.right_panel.instances, vec![files]);
        assert!(!app.state.terminals.contains_key(&logs.terminal_id));
    }

    #[test]
    fn command_exit_after_startup_hides_only_that_pane_panel() {
        let mut app = test_app();
        let (a, b) = split(&mut app);
        let a_files = install_instance(&mut app, a, RightPanelMode::Files, "/a");
        let _b_files = install_instance(&mut app, b, RightPanelMode::Files, "/b");
        app.state.right_panel.instances[0].spawned_at -= crate::right_panel::FAILED_START_WINDOW;
        app.state.right_panel.pane_mut(a).visible = true;
        app.state.right_panel.pane_mut(b).visible = true;

        assert!(app.right_panel_pane_died(a_files.pane_id));

        assert!(!app.state.right_panel.is_visible_for(Some(a)));
        assert!(app.state.right_panel.is_visible_for(Some(b)));
        assert!(!app.state.terminals.contains_key(&a_files.terminal_id));
        assert!(!app.right_panel_pane_died(a_files.pane_id));
    }

    #[test]
    fn set_width_is_global_and_survives_toggles_until_reset() {
        let mut app = test_app();
        app.set_right_panel_width(Some(40));
        app.toggle_right_panel();
        app.toggle_right_panel();
        let width = app.state.right_panel.width;
        app.apply_right_panel_config(width, &crate::config::RightPanelConfig::default());
        assert_eq!(app.state.right_panel.manual_width, Some(40));

        app.set_right_panel_width(None);
        assert_eq!(
            app.state.right_panel.effective_width(),
            app.state.right_panel.width
        );
    }

    #[tokio::test]
    async fn pane_selection_text_reads_right_panel_terminal() {
        let mut app = test_app();
        let owner = focused(&app);
        let instance = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        app.terminal_runtimes.insert(
            instance.terminal_id.clone(),
            TerminalRuntime::test_with_scrollback_bytes(20, 5, 1000, b"hello world"),
        );
        let params = crate::api::schema::PaneSelectionReadParams {
            pane_id: right_panel::public_id(&instance.terminal_id),
            anchor: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
            cursor: crate::api::schema::PaneTextPoint { row: 0, col: 4 },
            content_revision: None,
        };
        assert_eq!(app.pane_selection_text(&params).unwrap(), "hello");
    }

    #[test]
    fn right_panel_runtime_rejects_foreign_ids() {
        let mut app = test_app();
        let owner = focused(&app);
        let instance = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        assert!(app.right_panel_runtime("w1:p1").is_none());
        assert!(app
            .right_panel_runtime(&right_panel::public_id(&instance.terminal_id))
            .is_none());
    }
}
