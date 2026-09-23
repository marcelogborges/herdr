use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use tracing::warn;

use crate::app::App;
use crate::layout::PaneId;
use crate::pane::PaneLaunchEnv;
use crate::right_panel::{self, RightPanelInstance, RightPanelMode, MAX_INSTANCES};
use crate::terminal::{TerminalId, TerminalRuntime, TerminalState};

impl App {
    pub(crate) fn toggle_right_panel(&mut self) {
        self.release_exited_right_panel_instances();
        let panel = &mut self.state.right_panel;
        panel.visible = !panel.visible;
        panel.focused = panel.visible;
        self.request_right_panel_render();
    }

    pub(crate) fn show_right_panel(&mut self, mode: RightPanelMode) {
        self.release_exited_right_panel_instances();
        let panel = &mut self.state.right_panel;
        panel.mode = mode;
        panel.visible = true;
        panel.focused = true;
        self.request_right_panel_render();
    }

    fn release_exited_right_panel_instances(&mut self) {
        for instance in self.state.right_panel.take_exited() {
            self.release_right_panel_instance(instance);
        }
    }

    pub(crate) fn focus_right_panel(&mut self, terminal_id: &str) -> bool {
        let panel = &mut self.state.right_panel;
        let owns_active = panel.visible
            && panel
                .active
                .as_ref()
                .is_some_and(|active| active.as_str() == terminal_id);
        if owns_active && !panel.focused {
            panel.focused = true;
            self.request_right_panel_render();
        }
        owns_active
    }

    pub(crate) fn blur_right_panel(&mut self) {
        if std::mem::take(&mut self.state.right_panel.focused) {
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

    pub(crate) fn sync_right_panel(&mut self, panel: Rect) {
        if !self.state.right_panel.visible {
            return;
        }
        let target = self.right_panel_target_pane();
        let mode = self.state.right_panel.mode;
        let key = target.map(|(ws_idx, pane_id)| (ws_idx, pane_id, mode));
        if key.is_some()
            && key == self.state.right_panel.target
            && self.state.right_panel.active_instance().is_some()
        {
            return;
        }
        let dir = self.right_panel_dir(target);
        self.state.right_panel.target = key;
        if let Some(index) = self.state.right_panel.find(mode, &dir) {
            self.state.right_panel.activate(index);
            return;
        }
        let content = right_panel::content_rect(panel);
        match self.spawn_right_panel_instance(mode, dir, content.height, content.width) {
            Ok(instance) => {
                self.state.right_panel.instances.push(instance);
                let last = self.state.right_panel.instances.len() - 1;
                self.state.right_panel.activate(last);
                for evicted in self.state.right_panel.evict_over(MAX_INSTANCES) {
                    self.release_right_panel_instance(evicted);
                }
            }
            Err(err) => {
                warn!(err = %err, "right panel command failed to start");
                let panel = &mut self.state.right_panel;
                panel.visible = false;
                panel.focused = false;
                panel.target = None;
            }
        }
    }

    fn right_panel_target_pane(&self) -> Option<(usize, PaneId)> {
        let ws_idx = self.state.active?;
        let pane_id = self.state.workspaces.get(ws_idx)?.focused_pane_id()?;
        Some((ws_idx, pane_id))
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
        mode: RightPanelMode,
        dir: PathBuf,
        rows: u16,
        cols: u16,
    ) -> std::io::Result<RightPanelInstance> {
        let command = self.state.right_panel.command(mode).to_owned();
        let pane_id = PaneId::alloc();
        let terminal_id = TerminalId::alloc();
        let launch_env =
            PaneLaunchEnv::from_extra(self.custom_command_env().0).without_pane_identity();
        let runtime = TerminalRuntime::spawn_shell_command(
            pane_id,
            rows.max(1),
            cols.max(1),
            dir.clone(),
            &command,
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

    fn install_instance(app: &mut App, mode: RightPanelMode, dir: &str) -> RightPanelInstance {
        let instance = RightPanelInstance {
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

    #[test]
    fn toggle_shows_focused_then_hides_unfocused() {
        let mut app = test_app();

        app.toggle_right_panel();
        assert!(app.state.right_panel.visible && app.state.right_panel.focused);

        app.toggle_right_panel();
        assert!(!app.state.right_panel.visible && !app.state.right_panel.focused);
    }

    #[test]
    fn show_switches_mode_and_focuses() {
        let mut app = test_app();

        app.show_right_panel(RightPanelMode::Diff);

        assert_eq!(app.state.right_panel.mode, RightPanelMode::Diff);
        assert!(app.state.right_panel.visible && app.state.right_panel.focused);
    }

    #[test]
    fn focus_moves_between_panel_and_panes() {
        let mut app = test_app();
        let instance = install_instance(&mut app, RightPanelMode::Files, "/a");
        app.state.right_panel.activate(0);
        app.state.right_panel.visible = true;

        assert!(!app.focus_right_panel("term_other"));
        assert!(app.focus_right_panel(instance.terminal_id.as_str()));
        assert!(app.state.right_panel.focused);

        app.blur_right_panel();
        assert!(!app.state.right_panel.focused);
    }

    #[test]
    fn sync_reuses_the_instance_for_the_resolved_directory() {
        let mut app = test_app();
        let pane_id = app.state.workspaces[0].focused_pane_id().unwrap();
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        let expected_dir = app.right_panel_dir(Some((0, pane_id)));
        let _other = install_instance(&mut app, RightPanelMode::Files, "/elsewhere");
        let matching = install_instance(
            &mut app,
            RightPanelMode::Files,
            expected_dir.to_str().unwrap(),
        );
        app.state.right_panel.visible = true;

        app.sync_right_panel(Rect::new(60, 0, 40, 20));

        assert_eq!(
            app.state.right_panel.active.as_ref(),
            Some(&matching.terminal_id)
        );
        assert_eq!(
            app.state.right_panel.target,
            Some((0, pane_id, RightPanelMode::Files))
        );
        assert!(expected_dir.is_absolute() || expected_dir == home);
    }

    #[test]
    fn pane_died_for_active_instance_hides_and_releases_it() {
        let mut app = test_app();
        let instance = install_instance(&mut app, RightPanelMode::Diff, "/a");
        app.state.right_panel.instances[0].spawned_at -= crate::right_panel::FAILED_START_WINDOW;
        app.state.right_panel.activate(0);
        app.state.right_panel.visible = true;
        app.state.right_panel.focused = true;

        assert!(app.right_panel_pane_died(instance.pane_id));

        assert!(!app.state.right_panel.visible);
        assert!(app.state.right_panel.instances.is_empty());
        assert!(!app.state.terminals.contains_key(&instance.terminal_id));
        assert!(!app.right_panel_pane_died(instance.pane_id));
    }

    #[test]
    fn hide_show_hide_keeps_the_live_instance_for_reuse() {
        let mut app = test_app();
        let instance = install_instance(&mut app, RightPanelMode::Files, "/a");
        app.state.right_panel.activate(0);

        app.toggle_right_panel();
        assert!(app.state.right_panel.visible);
        app.toggle_right_panel();
        assert!(!app.state.right_panel.visible && !app.state.right_panel.focused);
        app.toggle_right_panel();
        assert!(app.state.right_panel.visible && app.state.right_panel.focused);
        assert_eq!(
            app.state.right_panel.active.as_ref(),
            Some(&instance.terminal_id)
        );
        app.toggle_right_panel();

        assert!(!app.state.right_panel.visible);
        assert_eq!(app.state.right_panel.instances, vec![instance.clone()]);
        assert!(app.state.terminals.contains_key(&instance.terminal_id));
    }

    #[test]
    fn failed_start_stays_visible_and_the_next_show_respawns() {
        let mut app = test_app();
        let files = install_instance(&mut app, RightPanelMode::Files, "/a");
        let diff = install_instance(&mut app, RightPanelMode::Diff, "/a");
        app.state.right_panel.activate(1);
        app.state.right_panel.mode = RightPanelMode::Diff;
        app.state.right_panel.visible = true;
        app.state.right_panel.target = Some((0, PaneId::alloc(), RightPanelMode::Diff));

        assert!(app.right_panel_pane_died(diff.pane_id));
        assert!(app.state.right_panel.visible);
        assert!(app.state.terminals.contains_key(&diff.terminal_id));

        app.show_right_panel(RightPanelMode::Files);

        assert!(app.state.right_panel.visible);
        assert_eq!(app.state.right_panel.mode, RightPanelMode::Files);
        assert_eq!(app.state.right_panel.target, None);
        assert_eq!(app.state.right_panel.instances, vec![files]);
        assert!(!app.state.terminals.contains_key(&diff.terminal_id));
    }

    #[test]
    fn failed_start_then_toggle_hides_and_the_next_toggle_shows_again() {
        let mut app = test_app();
        let diff = install_instance(&mut app, RightPanelMode::Diff, "/a");
        app.state.right_panel.activate(0);
        app.state.right_panel.mode = RightPanelMode::Diff;
        app.state.right_panel.visible = true;

        assert!(app.right_panel_pane_died(diff.pane_id));
        app.toggle_right_panel();
        assert!(!app.state.right_panel.visible);
        assert!(app.state.right_panel.instances.is_empty());

        app.toggle_right_panel();

        assert!(app.state.right_panel.visible);
        assert_eq!(app.state.right_panel.active, None);
    }

    #[test]
    fn command_exit_after_startup_hides_then_show_opens_fresh() {
        let mut app = test_app();
        let mut files = install_instance(&mut app, RightPanelMode::Files, "/a");
        files.spawned_at -= crate::right_panel::FAILED_START_WINDOW;
        app.state.right_panel.instances[0].spawned_at = files.spawned_at;
        app.state.right_panel.activate(0);
        app.state.right_panel.visible = true;

        assert!(app.right_panel_pane_died(files.pane_id));
        assert!(!app.state.right_panel.visible);
        assert!(app.state.right_panel.instances.is_empty());

        app.toggle_right_panel();

        assert!(app.state.right_panel.visible);
        assert_eq!(app.state.right_panel.target, None);
    }

    #[test]
    fn right_panel_runtime_rejects_foreign_ids() {
        let mut app = test_app();
        let instance = install_instance(&mut app, RightPanelMode::Files, "/a");
        assert!(app.right_panel_runtime("w1:p1").is_none());
        assert!(app
            .right_panel_runtime(&right_panel::public_id(&instance.terminal_id))
            .is_none());
    }
}
