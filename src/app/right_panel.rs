use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use tracing::warn;

use crate::app::state::AppState;
use crate::app::App;
use crate::layout::PaneId;
use crate::pane::PaneLaunchEnv;
use crate::right_panel::{
    self, RightPanelInstance, RightPanelMode, MAX_INSTANCES, PANEL_OWNER_PANE_ENV,
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
        let pane = self.state.right_panel.pane_mut(owner);
        pane.mode = mode;
        pane.visible = true;
        pane.focused = true;
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

    pub(crate) fn right_panel_pane_died(&mut self, pane_id: PaneId) -> bool {
        let Some(exit) = self
            .state
            .right_panel
            .command_exited(pane_id, std::time::Instant::now())
        else {
            return false;
        };
        if let crate::right_panel::PanelExit::Released(instance) = exit {
            self.release_right_panel_instance(instance);
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
            .chain(self.state.right_panel.instances.iter().map(|i| i.owner))
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
        if let Some(index) = self.state.right_panel.instance_for(owner, pane.mode) {
            self.state.right_panel.touch(index);
            return;
        }
        let dir = self.right_panel_dir(Some((target.workspace_index, owner)));
        let command = self.state.right_panel.command(pane.mode).to_owned();
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
                .instance_for(owner, RightPanelMode::Files)
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
            .take_instance(owner, RightPanelMode::Files)
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
        let content = right_panel::content_rect(panel);
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
            .and_then(|candidate| candidate.worktree_path.clone())
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
        let mut extra_env = self.custom_command_env().0;
        if let Some(owner_id) = self
            .find_pane(owner)
            .and_then(|(ws_idx, _)| self.public_pane_id(ws_idx, owner))
        {
            extra_env.push((PANEL_OWNER_PANE_ENV.to_owned(), owner_id));
        }
        let launch_env = PaneLaunchEnv::from_extra(extra_env).without_pane_identity();
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
        app.show_right_panel(RightPanelMode::Diff);
        app.state.focus_pane_in_workspace(0, b);
        app.show_right_panel(RightPanelMode::Files);

        assert_eq!(
            app.state.right_panel.pane(a).unwrap().mode,
            RightPanelMode::Diff
        );
        assert_eq!(
            app.state.right_panel.pane(b).unwrap().mode,
            RightPanelMode::Files
        );
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
        let files = install_instance(&mut app, owner, RightPanelMode::Files, "/a");
        let diff = install_instance(&mut app, owner, RightPanelMode::Diff, "/a");
        app.show_right_panel(RightPanelMode::Diff);

        assert!(app.right_panel_pane_died(diff.pane_id));
        assert!(app.state.right_panel.is_visible_for(Some(owner)));
        assert!(app.state.terminals.contains_key(&diff.terminal_id));

        app.show_right_panel(RightPanelMode::Files);

        assert_eq!(app.state.right_panel.instances, vec![files]);
        assert!(!app.state.terminals.contains_key(&diff.terminal_id));
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
        app.state
            .right_panel
            .apply_config(width, &crate::config::RightPanelConfig::default());
        assert_eq!(app.state.right_panel.manual_width, Some(40));

        app.set_right_panel_width(None);
        assert_eq!(
            app.state.right_panel.effective_width(),
            app.state.right_panel.width
        );
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
