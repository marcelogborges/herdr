use ratatui::layout::Rect;

use crate::app;
use crate::protocol::{self, FrameData};

#[cfg(test)]
pub(super) fn snapshot(
    app: &app::App,
    boot_id: &str,
    revision: u64,
    config_diagnostic: Option<&str>,
    location: Option<&crate::server::clients::ClientShellLocation>,
) -> protocol::ClientShellSnapshot {
    snapshot_with_completions(app, boot_id, revision, config_diagnostic, location).0
}

pub(super) fn snapshot_with_completions(
    app: &app::App,
    boot_id: &str,
    revision: u64,
    config_diagnostic: Option<&str>,
    location: Option<&crate::server::clients::ClientShellLocation>,
) -> (
    protocol::ClientShellSnapshot,
    protocol::endpoint::EndpointAgentCompletions,
) {
    let snapshot = app.session_metadata_snapshot();
    let completions = protocol::endpoint::EndpointAgentCompletions {
        boot_id: boot_id.to_owned(),
        revision,
        completions: snapshot
            .agents
            .iter()
            .filter_map(|agent| agent.completion_seq.map(|seq| (agent.pane_id.clone(), seq)))
            .collect(),
    };
    let focused_workspace_id = location
        .and_then(|location| location.focused_workspace_id.clone())
        .or_else(|| snapshot.focused_workspace_id.clone());
    let focused_tab_id = location
        .and_then(|location| location.focused_tab_id().map(str::to_owned))
        .or_else(|| snapshot.focused_tab_id.clone());
    let focused_panel_owner = focused_tab_id
        .as_deref()
        .and_then(|tab_id| app.parse_tab_id(tab_id))
        .and_then(|(workspace_index, tab_index)| {
            app.state.right_panel_owner(crate::ui::TabSurfaceTarget {
                workspace_index,
                tab_index,
            })
        });
    let focused_pane_id = focused_tab_id
        .as_deref()
        .and_then(|tab_id| app.parse_tab_id(tab_id))
        .and_then(|(workspace_index, tab_index)| {
            let pane_id = app
                .state
                .workspaces
                .get(workspace_index)?
                .tabs
                .get(tab_index)?
                .layout
                .focused();
            app.public_pane_id(workspace_index, pane_id)
        })
        .or_else(|| snapshot.focused_pane_id.clone());
    let workspaces = snapshot
        .workspaces
        .into_iter()
        .zip(&app.state.workspaces)
        .enumerate()
        .map(|(workspace_index, (workspace, state))| {
            let mut tokens = workspace.tokens.into_iter().collect::<Vec<_>>();
            tokens.sort_by(|left, right| left.0.cmp(&right.0));
            let workspace_id = workspace.workspace_id;
            let active_tab_id = location
                .and_then(|location| location.active_tab_ids.get(&workspace_id))
                .cloned()
                .unwrap_or(workspace.active_tab_id);
            let active_tab_index =
                app.parse_tab_id(&active_tab_id)
                    .and_then(|(tab_workspace_index, tab_index)| {
                        (tab_workspace_index == workspace_index).then_some(tab_index)
                    });
            protocol::ClientShellWorkspace {
                focused: focused_workspace_id.as_deref() == Some(workspace_id.as_str()),
                workspace_id,
                active_tab_id,
                new_workspace_cwd: app
                    .resolved_new_workspace_cwd_from_tab(workspace_index, active_tab_index)
                    .display()
                    .to_string(),
                number: workspace.number,
                label: workspace.label,
                custom_label: state.custom_name.is_some(),
                branch: state.branch(),
                git_ahead_behind: state.git_ahead_behind(),
                tokens,
                worktree: workspace
                    .worktree
                    .map(|worktree| protocol::ClientShellWorktree {
                        key: worktree.repo_key,
                        label: worktree.repo_name,
                        is_linked_worktree: worktree.is_linked_worktree,
                    }),
                agent_status: workspace.agent_status,
            }
        })
        .collect();
    let tabs = snapshot
        .tabs
        .into_iter()
        .zip(
            app.state
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.tabs.iter()),
        )
        .map(|(tab, state)| {
            let tab_id = tab.tab_id;
            protocol::ClientShellTab {
                focused: focused_tab_id.as_deref() == Some(tab_id.as_str()),
                tab_id,
                workspace_id: tab.workspace_id,
                number: tab.number,
                label: tab.label,
                custom_label: !state.is_auto_named(),
                zoomed: state.zoomed,
                agent_status: tab.agent_status,
            }
        })
        .collect();
    let panes = snapshot
        .panes
        .into_iter()
        .map(|pane| {
            let pane_id = pane.pane_id;
            let focused = focused_pane_id.as_deref() == Some(pane_id.as_str());
            let right_click_passthrough = app
                .parse_pane_id(&pane_id)
                .and_then(|(workspace_index, pane_id)| {
                    app.state
                        .workspaces
                        .get(workspace_index)?
                        .pane_state(pane_id)
                })
                .is_some_and(|pane| pane.right_click_passthrough);
            protocol::ClientShellPane {
                pane_id,
                workspace_id: pane.workspace_id,
                tab_id: pane.tab_id,
                label: pane.label,
                cwd: pane.cwd,
                foreground_cwd: pane.foreground_cwd,
                focused,
                right_click_passthrough,
            }
        })
        .collect();
    let live_claude_sessions = snapshot
        .agents
        .iter()
        .filter_map(|agent| {
            let session = agent.agent_session.as_ref()?;
            (session.agent == "claude"
                && session.kind == crate::agent_resume::AgentSessionRefKind::Id)
                .then(|| (session.value.clone(), agent.pane_id.clone()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let claude_sessions = app
        .claude_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|session| protocol::ClientShellClaudeSession {
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            cwd: session.cwd.clone(),
            context: session.context.clone(),
            updated_at_ms: session.updated_at_ms,
            pane_id: live_claude_sessions.get(&session.session_id).cloned(),
        })
        .collect();
    let agents = snapshot
        .agents
        .into_iter()
        .map(|agent| {
            let pane_id = agent.pane_id;
            let focused = focused_pane_id.as_deref() == Some(pane_id.as_str());
            let mut state_labels = agent.state_labels.into_iter().collect::<Vec<_>>();
            state_labels.sort_by(|left, right| left.0.cmp(&right.0));
            let mut tokens = agent.tokens.into_iter().collect::<Vec<_>>();
            tokens.sort_by(|left, right| left.0.cmp(&right.0));
            protocol::ClientShellAgent {
                pane_id,
                workspace_id: agent.workspace_id,
                tab_id: agent.tab_id,
                name: agent.name,
                display_agent: agent.display_agent,
                agent: agent.agent,
                title: agent.title,
                terminal_title: agent.terminal_title,
                terminal_title_stripped: agent.terminal_title_stripped,
                agent_status: agent.agent_status,
                state_change_seq: agent.state_change_seq,
                state_labels,
                tokens,
                focused,
            }
        })
        .collect();

    let agent_view_label = app
        .state
        .agent_view_override
        .as_ref()
        .map(|view| view.label.clone().unwrap_or_else(|| "filtered".to_owned()));
    let agent_order = crate::ui::agent_panel_entries_from(&app.state, &app.terminal_runtimes)
        .into_iter()
        .filter_map(|entry| app.public_pane_id(entry.ws_idx, entry.pane_id))
        .collect();

    let zoomed = focused_tab_id
        .as_deref()
        .and_then(|tab_id| app.parse_tab_id(tab_id))
        .and_then(|(workspace_index, tab_index)| {
            app.state
                .workspaces
                .get(workspace_index)?
                .tabs
                .get(tab_index)
        })
        .is_some_and(|tab| tab.zoomed);
    let tab_bar_right = app
        .state
        .tab_bar_right
        .iter()
        .filter_map(|segment| match segment {
            crate::app::state::TabBarStatusSegment::Zoom if zoomed => {
                Some(protocol::ClientShellTabStatusSegment {
                    text: "ZOOM".to_owned(),
                    accent: true,
                })
            }
            crate::app::state::TabBarStatusSegment::Text(Some(text)) if !text.is_empty() => {
                Some(protocol::ClientShellTabStatusSegment {
                    text: text.clone(),
                    accent: false,
                })
            }
            crate::app::state::TabBarStatusSegment::Zoom
            | crate::app::state::TabBarStatusSegment::Text(_) => None,
        })
        .collect();

    let product_announcement = app.state.product_announcement.as_ref().map(|announcement| {
        protocol::ClientShellProductAnnouncement {
            version: announcement.version.clone(),
            id: announcement.id.clone(),
            title: announcement.title.clone(),
            body: announcement.body.clone(),
            preview: announcement.preview,
        }
    });
    let release_notes =
        app.state
            .latest_release_notes
            .as_ref()
            .map(|notes| protocol::ClientShellReleaseNotes {
                version: notes.version.clone(),
                body: notes.body.clone(),
                preview: notes.preview,
            });

    let shell = protocol::ClientShellSnapshot {
        boot_id: boot_id.to_owned(),
        revision,
        config_diagnostic: config_diagnostic.map(str::to_owned),
        product_announcement,
        update_available: app.state.update_available.clone(),
        update_install_command: app.state.update_install_command.clone(),
        server_keybindings_toml: app.client_shell_keybindings_profile().map(str::to_owned),
        latest_release_notes_available: app.state.latest_release_notes_available,
        integration_updates_available: app.state.integration_updates_available(),
        worktree_directory: app.state.worktree_directory.to_string_lossy().into_owned(),
        release_notes,
        focused_workspace_id,
        focused_tab_id,
        focused_pane_id: app
            .state
            .right_panel
            .focused_public_id(focused_panel_owner)
            .or(focused_pane_id),
        tab_bar_right,
        tab_bar_right_separator: app.state.tab_bar_right_separator.clone(),
        agent_view_label,
        agent_order,
        workspaces,
        tabs,
        panes,
        agents,
        commands: app.client_shell_command_manifest(),
        claude_sessions,
        right_panel_tabs: app.state.right_panel.custom_labels(),
        right_panel: Some(right_panel_projection(app, focused_panel_owner)),
    };
    (shell, completions)
}

fn right_panel_projection(
    app: &app::App,
    owner: Option<crate::layout::PaneId>,
) -> protocol::ClientShellRightPanel {
    let state = &app.state.right_panel;
    let pane = owner.and_then(|owner| state.pane(owner));
    protocol::ClientShellRightPanel {
        views: state.views.clone(),
        activity_bar: state.bar_visible_for(owner),
        visible: pane.is_some_and(|pane| pane.visible),
        mode: pane.map(|pane| pane.mode.clone()).unwrap_or_default(),
        exited: owner
            .and_then(|owner| state.displayed(owner))
            .is_some_and(|instance| instance.exited),
    }
}

pub(super) struct RenderedPaneSurface {
    pub(super) frame: FrameData,
    pub(super) panes: Vec<protocol::PaneSurfacePane>,
    pub(super) splits: Vec<protocol::PaneSurfaceSplit>,
    pub(super) popup: Option<Box<protocol::ClientShellPopupSurface>>,
    pub(super) graphics: protocol::SurfaceGraphicsScene,
    pub(super) graphics_delivery: crate::kitty_graphics::surface::DeliveryCache,
    pub(super) graphics_sources: crate::kitty_graphics::surface::SourceFiles,
}

#[derive(Debug)]
pub(super) enum SurfaceRenderDeferred {
    Synchronized,
    Changed,
}

pub(super) fn render_pane_surface(
    app: &mut app::App,
    target: Option<crate::ui::TabSurfaceTarget>,
    area: Rect,
    resize_panes: bool,
    show_popup: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
    graphics_delivery: &crate::kitty_graphics::surface::DeliveryCache,
    client_id: u64,
) -> Result<RenderedPaneSurface, SurfaceRenderDeferred> {
    let panel_owner =
        |app: &app::App| target.and_then(|target| app.state.right_panel_owner(target));
    if let (Some(target), Some(panel)) = (
        target,
        app.state.right_panel.panel_rect(panel_owner(app), area),
    ) {
        app.sync_right_panel(target, panel);
    }
    let right_panel_owner = panel_owner(app);
    let right_panel = right_panel_owner.and_then(|owner| {
        app.state
            .right_panel
            .panel_rect(Some(owner), area)
            .map(|panel| (owner, panel))
    });
    let layout = crate::ui::compute_tab_surface_for(
        &app.state,
        &app.terminal_runtimes,
        target,
        area,
        resize_panes,
        cell_size,
    );
    let mut content_revisions_before = std::collections::HashMap::new();
    if let Some(target) = target {
        for pane in &layout.pane_infos {
            if let Some(runtime) = app.state.runtime_for_pane_in_workspace(
                &app.terminal_runtimes,
                target.workspace_index,
                pane.id,
            ) {
                let (synchronized, epoch) = runtime.synchronized_output_state();
                if synchronized {
                    return Err(SurfaceRenderDeferred::Synchronized);
                }
                let revision = runtime.content_seq();
                content_revisions_before.insert(pane.id, (epoch, revision));
            }
        }
    }
    let popup_revision_before = if show_popup {
        app.state
            .popup_pane
            .as_ref()
            .and_then(|popup| app.terminal_runtimes.get(&popup.terminal_id))
            .map(|runtime| {
                let (synchronized, epoch) = runtime.synchronized_output_state();
                if synchronized {
                    return Err(SurfaceRenderDeferred::Synchronized);
                }
                Ok(epoch)
            })
            .transpose()?
    } else {
        None
    };
    let (mut buffer, mut cursor, hyperlinks, layout) =
        crate::server::render_stream::render_tab_surface_virtual(
            &app.state,
            &app.terminal_runtimes,
            layout,
            area,
        );
    let right_panel_pane = right_panel.and_then(|(owner, panel)| {
        render_right_panel(app, &mut buffer, &mut cursor, owner, panel, cell_size)
    });
    if let Some(bar) = app.state.right_panel.bar_rect(right_panel_owner, area) {
        render_activity_bar(app, &mut buffer, right_panel_owner, bar);
    }
    let right_panel_focused = right_panel_pane.as_ref().is_some_and(|pane| pane.focused);
    let mut panes: Vec<protocol::PaneSurfacePane> = target
        .map(|target| {
            let workspace_index = target.workspace_index;
            layout
                .pane_infos
                .iter()
                .filter_map(|pane| {
                    app.public_pane_id(workspace_index, pane.id).map(|pane_id| {
                        let runtime = app.state.runtime_for_pane_in_workspace(
                            &app.terminal_runtimes,
                            workspace_index,
                            pane.id,
                        );
                        let mouse_reporting =
                            runtime.is_some_and(|runtime| runtime.mouse_reporting_enabled());
                        let sgr_pixel_mouse =
                            runtime.is_some_and(|runtime| runtime.sgr_pixel_mouse_enabled());
                        let (pixel_width, pixel_height) = if cell_size.is_known() {
                            (
                                u32::from(pane.inner_rect.width) * cell_size.width_px,
                                u32::from(pane.inner_rect.height) * cell_size.height_px,
                            )
                        } else {
                            (0, 0)
                        };
                        let content_revision = runtime.map_or(0, |runtime| {
                            let after = runtime.content_seq();
                            if content_revisions_before
                                .get(&pane.id)
                                .is_some_and(|&(_, before)| before == after)
                                && after.is_multiple_of(2)
                            {
                                after
                            } else {
                                after | 1
                            }
                        });
                        protocol::PaneSurfacePane {
                            pane_id,
                            content_revision,
                            rect: pane.rect.into(),
                            inner_rect: pane.inner_rect.into(),
                            scrollbar_rect: pane.scrollbar_rect.map(Into::into),
                            scroll: runtime.and_then(|runtime| runtime.scroll_metrics()).map(
                                |metrics| protocol::PaneSurfaceScrollMetrics {
                                    offset_from_bottom: metrics.offset_from_bottom as u64,
                                    max_offset_from_bottom: metrics.max_offset_from_bottom as u64,
                                    viewport_rows: metrics.viewport_rows as u64,
                                },
                            ),
                            focused: pane.is_focused && !right_panel_focused,
                            mouse_reporting,
                            sgr_pixel_mouse,
                            alternate_screen_active: runtime
                                .is_some_and(|runtime| runtime.alternate_screen_active()),
                            pixel_width,
                            pixel_height,
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    panes.extend(right_panel_pane);
    let pane_frames = layout
        .pane_infos
        .iter()
        .map(|pane| pane.rect)
        .collect::<Vec<_>>();
    let splits = layout
        .split_borders
        .iter()
        .filter_map(|split| {
            let hit_rect = split_hit_rect(
                split,
                app.state.pane_borders.draws_borders(),
                app.state.pane_gaps,
                &pane_frames,
            )?;
            let direction = match split.direction {
                ratatui::layout::Direction::Horizontal => {
                    protocol::PaneSurfaceSplitDirection::Horizontal
                }
                ratatui::layout::Direction::Vertical => {
                    protocol::PaneSurfaceSplitDirection::Vertical
                }
            };
            Some(protocol::PaneSurfaceSplit {
                direction,
                pos: split.pos,
                area: split.area.into(),
                hit_rect: hit_rect.into(),
                path: split.path.clone(),
            })
        })
        .collect();
    let popup = show_popup
        .then(|| render_popup_surface(app, area, resize_panes, cell_size))
        .flatten();
    let (graphics, next_graphics_delivery, graphics_sources) =
        crate::server::client_shell_graphics::collect(
            app,
            &layout.pane_infos,
            &layout.split_borders,
            popup.as_deref(),
            target,
            cell_size,
            graphics_delivery,
            client_id,
        );
    if let Some(target) = target {
        for (&pane_id, &(epoch, _)) in &content_revisions_before {
            if let Some(runtime) = app.state.runtime_for_pane_in_workspace(
                &app.terminal_runtimes,
                target.workspace_index,
                pane_id,
            ) {
                let (synchronized, after_epoch) = runtime.synchronized_output_state();
                if synchronized {
                    return Err(SurfaceRenderDeferred::Synchronized);
                }
                if after_epoch != epoch {
                    return Err(SurfaceRenderDeferred::Changed);
                }
            }
        }
    }
    if let Some(before) = popup_revision_before {
        if let Some(runtime) = app
            .state
            .popup_pane
            .as_ref()
            .and_then(|popup| app.terminal_runtimes.get(&popup.terminal_id))
        {
            let (synchronized, after_epoch) = runtime.synchronized_output_state();
            if synchronized {
                return Err(SurfaceRenderDeferred::Synchronized);
            }
            if after_epoch != before {
                return Err(SurfaceRenderDeferred::Changed);
            }
        }
    }
    Ok(RenderedPaneSurface {
        frame: FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, cursor, &hyperlinks),
        panes,
        splits,
        popup,
        graphics,
        graphics_delivery: next_graphics_delivery,
        graphics_sources,
    })
}

fn render_right_panel(
    app: &app::App,
    buffer: &mut ratatui::buffer::Buffer,
    cursor: &mut Option<protocol::CursorState>,
    owner: crate::layout::PaneId,
    panel: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Option<protocol::PaneSurfacePane> {
    use ratatui::style::Style;

    let state = &app.state.right_panel;
    let pane_state = state.pane(owner)?;
    let instance = state.displayed(owner)?;
    let runtime = app.terminal_runtimes.get(&instance.terminal_id)?;
    let panel = panel.intersection(buffer.area);
    let mode = &pane_state.mode;
    let fallback_view;
    let view = match state.view_for(mode) {
        Some(view) => view,
        None => {
            fallback_view = crate::right_panel::RightPanelView {
                label: mode.label().to_owned(),
                icon: String::new(),
                group: false,
                tabs: Vec::new(),
            };
            &fallback_view
        }
    };
    let content = crate::right_panel::content_rect(panel, view.header_rows());
    if content.width == 0 || content.height == 0 {
        return None;
    }
    let starting = !instance.exited
        && runtime.content_seq() == 0
        && instance.spawned_at.elapsed() < crate::right_panel::FAILED_START_WINDOW;
    resize_right_panel_terminal(app, instance, runtime, content, cell_size);
    if instance.exited || starting {
        clear_rect(buffer, content, Style::default());
        let label = view
            .tab(mode)
            .map_or_else(|| mode.label().to_owned(), |tab| tab.label.clone());
        render_right_panel_card(app, buffer, content, instance, &label);
    } else {
        let (panel_buffer, panel_cursor) = crate::server::render_stream::render_terminal_virtual(
            runtime,
            Rect::new(0, 0, content.width, content.height),
        );
        for y in 0..content.height {
            for x in 0..content.width {
                buffer[(content.x + x, content.y + y)] = panel_buffer[(x, y)].clone();
            }
        }
        if pane_state.focused {
            *cursor = panel_cursor.map(|mut panel_cursor| {
                panel_cursor.x = panel_cursor.x.saturating_add(content.x);
                panel_cursor.y = panel_cursor.y.saturating_add(content.y);
                panel_cursor
            });
        }
    }

    let palette = &app.state.palette;
    let focused = pane_state.focused;
    let edge = Style::default().fg(if !focused {
        palette.overlay0
    } else if state.is_prod(mode) {
        palette.red
    } else {
        palette.accent
    });
    for y in panel.y..panel.y + panel.height {
        let cell = &mut buffer[(panel.x, y)];
        cell.reset();
        cell.set_symbol("│");
        cell.set_style(edge);
    }
    render_right_panel_header(app, buffer, owner, panel, view, instance);

    let (pixel_width, pixel_height) = if cell_size.is_known() {
        (
            u32::from(content.width) * cell_size.width_px,
            u32::from(content.height) * cell_size.height_px,
        )
    } else {
        (0, 0)
    };
    Some(protocol::PaneSurfacePane {
        pane_id: crate::right_panel::public_id(&instance.terminal_id),
        content_revision: runtime.content_seq() | 1,
        rect: panel.into(),
        inner_rect: content.into(),
        scrollbar_rect: None,
        scroll: runtime
            .scroll_metrics()
            .map(|metrics| protocol::PaneSurfaceScrollMetrics {
                offset_from_bottom: metrics.offset_from_bottom as u64,
                max_offset_from_bottom: metrics.max_offset_from_bottom as u64,
                viewport_rows: metrics.viewport_rows as u64,
            }),
        focused,
        mouse_reporting: runtime.mouse_reporting_enabled(),
        sgr_pixel_mouse: runtime.sgr_pixel_mouse_enabled(),
        alternate_screen_active: runtime.alternate_screen_active(),
        pixel_width,
        pixel_height,
    })
}

fn clear_rect(buffer: &mut ratatui::buffer::Buffer, rect: Rect, style: ratatui::style::Style) {
    let rect = rect.intersection(buffer.area);
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let cell = &mut buffer[(x, y)];
            cell.reset();
            cell.set_symbol(" ");
            cell.set_style(style);
        }
    }
}

/// Writes `text` from `x` without passing `right`; returns the column after it.
fn put_text(
    buffer: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    right: u16,
    text: &str,
    style: ratatui::style::Style,
) -> u16 {
    if x >= right {
        return x;
    }
    let (end, _) = buffer.set_stringn(x, y, text, usize::from(right - x), style);
    end
}

fn render_right_panel_header(
    app: &app::App,
    buffer: &mut ratatui::buffer::Buffer,
    owner: crate::layout::PaneId,
    panel: Rect,
    view: &crate::right_panel::RightPanelView,
    instance: &crate::right_panel::RightPanelInstance,
) {
    use ratatui::style::{Modifier, Style};

    let palette = &app.state.palette;
    let state = &app.state.right_panel;
    let Some(pane_state) = state.pane(owner) else {
        return;
    };
    let header = Style::default().bg(palette.panel_bg).fg(palette.overlay0);
    let rows = view.header_rows().min(panel.height);
    clear_rect(
        buffer,
        Rect::new(panel.x + 1, panel.y, panel.width.saturating_sub(1), rows),
        header,
    );
    let layout = crate::right_panel::header_layout(panel, view, &pane_state.mode);
    let right = layout
        .close
        .map_or(panel.x + panel.width, |close| close.x.saturating_sub(1));
    let mut x = panel.x + 2;
    if !view.icon.is_empty() {
        x = put_text(
            buffer,
            x,
            panel.y,
            right,
            &view.icon,
            header.fg(palette.accent),
        );
        x = put_text(buffer, x, panel.y, right, " ", header);
    }
    x = put_text(
        buffer,
        x,
        panel.y,
        right,
        &view.label.to_uppercase(),
        header.fg(palette.text).add_modifier(Modifier::BOLD),
    );
    if pane_state.mode == crate::right_panel::RightPanelMode::Files {
        let mut parts = instance
            .dir
            .components()
            .rev()
            .filter_map(|part| match part {
                std::path::Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .take(2)
            .collect::<Vec<_>>();
        parts.reverse();
        if let Some((last, parents)) = parts.split_last() {
            x = put_text(buffer, x, panel.y, right, "  ", header);
            for parent in parents {
                x = put_text(
                    buffer,
                    x,
                    panel.y,
                    right,
                    parent,
                    header.fg(palette.overlay1),
                );
                x = put_text(buffer, x, panel.y, right, " › ", header);
            }
            put_text(buffer, x, panel.y, right, last, header.fg(palette.subtext0));
        }
    }
    if let Some(close) = layout.close {
        buffer.set_string(close.x, close.y, " ✕ ", header.fg(palette.overlay1));
    }

    let tab_right = panel.x + panel.width;
    for (mode, rect) in &layout.tabs {
        let Some(tab) = view.tab(mode) else {
            continue;
        };
        let tab_instance = state
            .instance_for(owner, mode)
            .map(|index| &state.instances[index]);
        let (dot, dot_color) = match tab_instance {
            Some(instance) if instance.exited => ("✕", palette.red),
            Some(_) => ("●", palette.green),
            None => ("○", palette.overlay0),
        };
        let mut x = put_text(buffer, rect.x, rect.y, tab_right, dot, header.fg(dot_color));
        x = put_text(buffer, x, rect.y, tab_right, " ", header);
        let label_style = if *mode == pane_state.mode {
            header
                .fg(if tab.prod {
                    palette.red
                } else {
                    palette.accent
                })
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            header.fg(palette.overlay1)
        };
        x = put_text(buffer, x, rect.y, tab_right, &tab.label, label_style);
        if tab.prod {
            x = put_text(buffer, x, rect.y, tab_right, " ", header);
            put_text(
                buffer,
                x,
                rect.y,
                tab_right,
                crate::right_panel::prod_badge(),
                Style::default()
                    .bg(palette.red)
                    .fg(ratatui::style::Color::Black)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
    if let Some((rect, _, hidden)) = &layout.overflow {
        put_text(
            buffer,
            rect.x,
            rect.y,
            tab_right,
            &format!("›{hidden}"),
            header.fg(palette.overlay1),
        );
    }
}

fn render_right_panel_card(
    app: &app::App,
    buffer: &mut ratatui::buffer::Buffer,
    content: Rect,
    instance: &crate::right_panel::RightPanelInstance,
    label: &str,
) {
    use ratatui::style::{Modifier, Style};

    let palette = &app.state.palette;
    let lines: Vec<Vec<(String, Style)>> = if instance.exited {
        let mut lines = vec![vec![
            ("✕  ".to_owned(), Style::default().fg(palette.red)),
            (
                format!("{label} exited"),
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD),
            ),
        ]];
        if let Some(at) = &instance.exited_at {
            lines.push(vec![(at.clone(), Style::default().fg(palette.overlay1))]);
        }
        lines.push(Vec::new());
        let key = Style::default().fg(palette.text).bg(palette.surface0);
        let hint = Style::default().fg(palette.subtext0);
        lines.push(vec![
            (" enter ".to_owned(), key),
            (" reopen   ".to_owned(), hint),
            (" esc ".to_owned(), key),
            (" close".to_owned(), hint),
        ]);
        lines
    } else {
        vec![vec![
            ("⠋ ".to_owned(), Style::default().fg(palette.accent)),
            (
                format!("starting {label}…"),
                Style::default().fg(palette.subtext0),
            ),
        ]]
    };
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let top = content.y + content.height.saturating_sub(height) / 2;
    let right = content.x + content.width;
    for (row, line) in lines.iter().enumerate() {
        let y = top + u16::try_from(row).unwrap_or(0);
        if y >= content.y + content.height {
            break;
        }
        let width: usize = line
            .iter()
            .map(|(text, _)| unicode_width::UnicodeWidthStr::width(text.as_str()))
            .sum();
        let width = u16::try_from(width).unwrap_or(u16::MAX);
        let mut x = content.x + content.width.saturating_sub(width) / 2;
        for (text, style) in line {
            x = put_text(buffer, x, y, right, text, *style);
        }
    }
}

fn render_activity_bar(
    app: &app::App,
    buffer: &mut ratatui::buffer::Buffer,
    owner: Option<crate::layout::PaneId>,
    bar: Rect,
) {
    use ratatui::style::{Modifier, Style};

    let palette = &app.state.palette;
    let state = &app.state.right_panel;
    let bar = bar.intersection(buffer.area);
    clear_rect(buffer, bar, Style::default().bg(palette.panel_bg));
    let active = owner
        .and_then(|owner| state.pane(owner))
        .filter(|pane| pane.visible)
        .and_then(|pane| crate::right_panel::view_index(&state.views, &pane.mode));
    let icons = crate::right_panel::activity_bar_icons(bar, state.views.len());
    for (index, (view, rect)) in state.views.iter().zip(icons).enumerate() {
        let right = rect.x + rect.width;
        let middle = rect.y + rect.height / 2;
        // icons are one cell; Nerd Font glyphs spill into the next one, so the centre column
        // leaves room on both sides
        let icon_x = rect.x + rect.width / 2;
        if active == Some(index) {
            let tile = Style::default().bg(palette.surface0);
            clear_rect(buffer, rect, tile);
            for y in rect.y..rect.y + rect.height {
                put_text(buffer, rect.x, y, right, "▌", tile.fg(palette.accent));
            }
            put_text(
                buffer,
                icon_x,
                middle,
                right,
                &view.icon,
                tile.fg(palette.accent).add_modifier(Modifier::BOLD),
            );
        } else {
            put_text(
                buffer,
                icon_x,
                middle,
                right,
                &view.icon,
                Style::default().bg(palette.panel_bg).fg(palette.subtext0),
            );
        }
    }
}

fn resize_right_panel_terminal(
    app: &app::App,
    instance: &crate::right_panel::RightPanelInstance,
    runtime: &crate::terminal::TerminalRuntime,
    content: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    if !app
        .state
        .direct_attach_resize_locks
        .contains(&instance.terminal_id)
    {
        runtime.resize(
            content.height,
            content.width,
            cell_size.width_px,
            cell_size.height_px,
        );
    }
}

pub(super) fn resize_right_panel_runtime(
    app: &app::App,
    target: crate::ui::TabSurfaceTarget,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    let Some(owner) = app.state.right_panel_owner(target) else {
        return;
    };
    let Some(panel) = app.state.right_panel.panel_rect(Some(owner), area) else {
        return;
    };
    let Some(instance) = app.state.right_panel.displayed(owner) else {
        return;
    };
    let Some(runtime) = app.terminal_runtimes.get(&instance.terminal_id) else {
        return;
    };
    let mode = app
        .state
        .right_panel
        .pane(owner)
        .map(|pane| pane.mode.clone())
        .unwrap_or_default();
    let content = crate::right_panel::content_rect(panel, app.state.right_panel.header_rows(&mode));
    if content.width > 0 && content.height > 0 {
        resize_right_panel_terminal(app, instance, runtime, content, cell_size);
    }
}

fn render_popup_surface(
    app: &app::App,
    area: Rect,
    resize_runtime: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Option<Box<protocol::ClientShellPopupSurface>> {
    let popup = app.state.popup_pane.as_ref()?;
    let geometry = if resize_runtime {
        resize_popup_runtime(app, area, cell_size)?
    } else {
        crate::popup_size::resolve_popup_geometry(popup.width, popup.height, area)?
    };
    let runtime = app.terminal_runtimes.get(&popup.terminal_id)?;
    let content_area = Rect::new(0, 0, geometry.inner.width, geometry.inner.height);
    let (buffer, cursor) =
        crate::server::render_stream::render_terminal_virtual(runtime, content_area);
    let hyperlinks = runtime.visible_hyperlinks(content_area);
    let title = app
        .state
        .terminals
        .get(&popup.terminal_id)
        .and_then(|terminal| terminal.manual_label.clone())
        .unwrap_or_else(|| "popup".to_owned());
    let (pixel_width, pixel_height) = if cell_size.is_known() {
        (
            u32::from(content_area.width) * cell_size.width_px,
            u32::from(content_area.height) * cell_size.height_px,
        )
    } else {
        (0, 0)
    };
    Some(Box::new(protocol::ClientShellPopupSurface {
        terminal_id: popup.terminal_id.to_string(),
        title,
        width: popup.width.map(client_popup_size),
        height: popup.height.map(client_popup_size),
        frame: FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, cursor, &hyperlinks),
        mouse_reporting: runtime.mouse_reporting_enabled(),
        sgr_pixel_mouse: runtime.sgr_pixel_mouse_enabled(),
        pixel_width,
        pixel_height,
    }))
}

pub(super) fn resize_popup_runtime(
    app: &app::App,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Option<crate::popup_size::PopupResolvedGeometry> {
    let popup = app.state.popup_pane.as_ref()?;
    let geometry = crate::popup_size::resolve_popup_geometry(popup.width, popup.height, area)?;
    let runtime = app.terminal_runtimes.get(&popup.terminal_id)?;
    if !app
        .state
        .direct_attach_resize_locks
        .contains(&popup.terminal_id)
    {
        runtime.resize(
            geometry.inner.height,
            geometry.inner.width,
            cell_size.width_px,
            cell_size.height_px,
        );
    }
    Some(geometry)
}

fn client_popup_size(size: crate::popup_size::PopupSize) -> protocol::ClientShellPopupSize {
    match size {
        crate::popup_size::PopupSize::Cells(cells) => protocol::ClientShellPopupSize::Cells(cells),
        crate::popup_size::PopupSize::Percent(percent) => {
            protocol::ClientShellPopupSize::Percent(percent)
        }
    }
}

fn split_hit_rect(
    split: &crate::layout::SplitBorder,
    pane_borders: bool,
    pane_gaps: bool,
    pane_frames: &[Rect],
) -> Option<Rect> {
    let hit = match (split.direction, pane_borders, pane_gaps) {
        (ratatui::layout::Direction::Horizontal, true, false) => {
            Rect::new(split.pos, split.area.y, 1, split.area.height)
        }
        (ratatui::layout::Direction::Horizontal, true, true) => {
            let start = split.pos.saturating_sub(1);
            Rect::new(
                start,
                split.area.y,
                split.pos.saturating_sub(start).saturating_add(1),
                split.area.height,
            )
        }
        (ratatui::layout::Direction::Horizontal, false, true) => Rect::new(
            split.pos.checked_sub(1)?,
            split.area.y,
            1,
            split.area.height,
        ),
        (ratatui::layout::Direction::Vertical, true, false) => {
            Rect::new(split.area.x, split.pos, split.area.width, 1)
        }
        (ratatui::layout::Direction::Vertical, true, true) => {
            let start = split.pos.saturating_sub(1);
            Rect::new(
                split.area.x,
                start,
                split.area.width,
                split.pos.saturating_sub(start).saturating_add(1),
            )
        }
        (ratatui::layout::Direction::Vertical, false, true) => {
            Rect::new(split.area.x, split.pos.checked_sub(1)?, split.area.width, 1)
        }
        (_, false, false) => return None,
    };
    if !pane_borders
        && pane_frames.iter().any(|pane| {
            hit.x < pane.right()
                && hit.right() > pane.x
                && hit.y < pane.bottom()
                && hit.bottom() > pane.y
        })
    {
        return None;
    }
    Some(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_app(mode: &str, focused: bool) -> (app::App, crate::right_panel::RightPanelInstance) {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![crate::workspace::Workspace::test_new("panel")];
        app.state.active = Some(0);
        let tab = |label: &str, group: Option<&str>, tag: Option<&str>| {
            crate::config::RightPanelTabConfig {
                label: label.into(),
                command: "true".into(),
                icon: None,
                group: group.map(str::to_owned),
                tag: tag.map(str::to_owned),
                shared: false,
            }
        };
        let config = crate::config::RightPanelConfig {
            activity_bar: crate::config::RightPanelActivityBarConfig::Always,
            tabs: vec![
                tab("vigias", None, None),
                tab("rails c", Some("ops"), Some("prod")),
                tab("awsx", Some("ops"), None),
            ],
            ..Default::default()
        };
        let width = app.state.right_panel.width;
        app.apply_right_panel_config(width, &config);
        let owner = app.state.workspaces[0].focused_pane_id().unwrap();
        let mode = crate::right_panel::RightPanelMode::from_label(mode);
        let instance = crate::right_panel::RightPanelInstance {
            owner,
            mode: mode.clone(),
            dir: std::path::PathBuf::from("/home/user/projects/vakinha-api"),
            pane_id: crate::layout::PaneId::alloc(),
            terminal_id: crate::terminal::TerminalId::alloc(),
            spawned_at: std::time::Instant::now(),
            exited: false,
            exited_at: None,
        };
        app.terminal_runtimes.insert(
            instance.terminal_id.clone(),
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(40, 18, 0, b""),
        );
        app.state.right_panel.instances.push(instance.clone());
        app.state.right_panel.set_mode(owner, mode);
        let pane = app.state.right_panel.pane_mut(owner);
        pane.visible = true;
        pane.focused = focused;
        (app, instance)
    }

    const PANEL: Rect = Rect::new(40, 0, 40, 20);

    fn render_panel(
        app: &app::App,
        owner: crate::layout::PaneId,
    ) -> (ratatui::buffer::Buffer, protocol::PaneSurfacePane) {
        let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 83, 20));
        let mut cursor = None;
        let pane = render_right_panel(
            app,
            &mut buffer,
            &mut cursor,
            owner,
            PANEL,
            crate::kitty_graphics::HostCellSize::default(),
        )
        .expect("rendered panel");
        (buffer, pane)
    }

    fn row(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
        (buffer.area.x..buffer.area.x + buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn group_view_header_has_a_title_and_a_tab_row_with_a_prod_badge() {
        let (app, instance) = panel_app("rails c", true);
        let (buffer, pane) = render_panel(&app, instance.owner);

        assert!(row(&buffer, 0).contains("R OPS"), "{:?}", row(&buffer, 0));
        assert!(row(&buffer, 0).contains(" ✕ "));
        let tabs = row(&buffer, 1);
        assert!(tabs.contains("● rails c  PROD"), "{tabs:?}");
        assert!(tabs.contains("○ awsx"), "{tabs:?}");
        assert_eq!(pane.inner_rect, Rect::new(41, 2, 39, 18).into());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn focused_prod_tab_draws_a_red_edge_and_other_tabs_the_accent() {
        let (app, instance) = panel_app("rails c", true);
        let (buffer, _) = render_panel(&app, instance.owner);
        assert_eq!(buffer[(40, 5)].fg, app.state.palette.red);

        let (app, instance) = panel_app("vigias", true);
        let (buffer, _) = render_panel(&app, instance.owner);
        assert_eq!(buffer[(40, 5)].fg, app.state.palette.accent);

        let (app, instance) = panel_app("rails c", false);
        let (buffer, _) = render_panel(&app, instance.owner);
        assert_eq!(buffer[(40, 5)].fg, app.state.palette.overlay0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn files_title_shows_a_breadcrumb_of_the_panel_directory() {
        let (app, instance) = panel_app("files", false);
        let (buffer, pane) = render_panel(&app, instance.owner);
        let title = row(&buffer, 0);
        assert!(
            title.contains("F FILES  projects › vakinha-api"),
            "{title:?}"
        );
        assert_eq!(pane.inner_rect, Rect::new(41, 1, 39, 19).into());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exited_and_starting_commands_show_a_centered_card() {
        let (mut app, instance) = panel_app("vigias", true);
        let text = |buffer: &ratatui::buffer::Buffer| {
            (0..buffer.area.height)
                .map(|y| row(buffer, y))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let (buffer, _) = render_panel(&app, instance.owner);
        assert!(
            text(&buffer).contains("⠋ starting vigias…"),
            "{}",
            text(&buffer)
        );

        app.state.right_panel.instances[0].exited = true;
        app.state.right_panel.instances[0].exited_at = Some("14:32".into());
        let (buffer, _) = render_panel(&app, instance.owner);
        let card = text(&buffer);
        assert!(card.contains("✕  vigias exited"), "{card}");
        assert!(card.contains("14:32"));
        assert!(card.contains(" enter  reopen    esc  close"), "{card}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn activity_bar_marks_the_shown_view() {
        let (app, instance) = panel_app("awsx", true);
        let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 85, 20));
        render_activity_bar(
            &app,
            &mut buffer,
            Some(instance.owner),
            Rect::new(80, 0, 5, 20),
        );

        // tiles start on rows 1, 5, 9; icon on the middle row, centre column
        let icons = [2u16, 6, 10]
            .iter()
            .map(|&y| {
                (
                    buffer[(80, y)].symbol().to_owned(),
                    buffer[(82, y)].symbol().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            icons,
            vec![
                (" ".to_owned(), "F".to_owned()),
                (" ".to_owned(), "V".to_owned()),
                ("▌".to_owned(), "R".to_owned()),
            ]
        );
        assert_eq!(buffer[(82, 10)].fg, app.state.palette.accent);
        assert_eq!(buffer[(81, 10)].bg, app.state.palette.surface0);
        assert_eq!(buffer[(80, 9)].symbol(), "▌");
        assert_eq!(buffer[(82, 2)].fg, app.state.palette.subtext0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_projects_views_and_the_focused_pane_panel() {
        let (mut app, _) = panel_app("awsx", true);
        app.state.right_panel.instances[0].exited = true;
        let projection = snapshot(&app, "boot", 1, None, None)
            .right_panel
            .expect("right panel projection");
        assert_eq!(projection.views.len(), 3);
        assert!(projection.activity_bar && projection.visible && projection.exited);
        assert_eq!(
            projection.mode,
            crate::right_panel::RightPanelMode::Custom("awsx".into())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_metadata_skips_scroll_reads_and_preserves_public_session_data() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let mut workspace = crate::workspace::Workspace::test_new("active");
        workspace.test_add_tab(None);
        app.state.workspaces = vec![workspace, crate::workspace::Workspace::test_new("hidden")];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();
        let history = (0..40)
            .map(|line| format!("line-{line}\r\n"))
            .collect::<String>();
        for (index, terminal) in app.state.terminals.values_mut().enumerate() {
            if index < 2 {
                terminal.set_detected_state(
                    Some(crate::detect::Agent::Pi),
                    crate::detect::AgentState::Working,
                );
                terminal.agent_name = Some(format!("agent-{index}"));
                terminal.last_agent_state_change_seq = Some(5);
                terminal.last_agent_completion_seq = Some(7);
            }
            let runtime = crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                20,
                5,
                1000,
                history.as_bytes(),
            );
            runtime.scroll_up(3);
            app.terminal_runtimes.insert(terminal.id.clone(), runtime);
        }

        let metadata = app.session_metadata_snapshot();
        assert_eq!(metadata.panes.len(), 3);
        assert_eq!(metadata.agents.len(), 2);
        assert!(metadata.panes.iter().all(|pane| pane.scroll.is_none()));
        assert!(app
            .terminal_runtimes
            .values()
            .all(|runtime| runtime.test_scroll_metrics_reads() == 0));

        let mut public = app.session_snapshot();
        for pane in &mut public.panes {
            let scroll = pane
                .scroll
                .take()
                .expect("public session must expose scroll");
            assert_eq!(scroll.offset_from_bottom, 3);
            assert!(scroll.max_offset_from_bottom >= 3);
            assert_eq!(scroll.viewport_rows, 5);
        }
        assert_eq!(metadata, public);
        assert!(app
            .terminal_runtimes
            .values()
            .all(|runtime| runtime.test_scroll_metrics_reads() == 1));

        let (shell, completions) = snapshot_with_completions(&app, "boot", 9, None, None);
        assert_eq!(shell.panes.len(), 3);
        assert_eq!(shell.agents.len(), 2);
        assert_eq!(shell.focused_pane_id, metadata.focused_pane_id);
        for (pane, public_pane) in shell.panes.iter().zip(&metadata.panes) {
            assert_eq!(pane.pane_id, public_pane.pane_id);
            assert_eq!(pane.workspace_id, public_pane.workspace_id);
            assert_eq!(pane.tab_id, public_pane.tab_id);
            assert_eq!(pane.cwd, public_pane.cwd);
        }
        for (agent, public_agent) in shell.agents.iter().zip(&metadata.agents) {
            assert_eq!(agent.pane_id, public_agent.pane_id);
            assert_eq!(agent.name, public_agent.name);
            assert_eq!(agent.agent_status, public_agent.agent_status);
            assert_eq!(agent.state_change_seq, 5);
        }
        assert_eq!(completions.revision, 9);
        assert_eq!(completions.completions.len(), 2);
        assert!(completions.completions.iter().all(|(_, seq)| *seq == 7));
        assert!(app
            .terminal_runtimes
            .values()
            .all(|runtime| runtime.test_scroll_metrics_reads() == 1));
    }

    #[test]
    fn snapshot_projects_cached_release_and_update_facts() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.integration_recommendations.clear();
        app.state.update_available = Some("0.8.3".into());
        app.state.update_install_command = "herdr update".into();
        app.state.latest_release_notes_available = true;
        app.state.latest_release_notes = Some(crate::release_notes::ReleaseNotes {
            version: "0.8.3".into(),
            body: "### Changed\n- Client shell".into(),
            preview: true,
        });

        let snapshot = snapshot(&app, "boot", 7, None, None);

        assert_eq!(snapshot.update_available.as_deref(), Some("0.8.3"));
        assert_eq!(snapshot.update_install_command, "herdr update");
        assert!(snapshot.latest_release_notes_available);
        assert!(!snapshot.integration_updates_available);
        assert_eq!(
            snapshot.release_notes.as_ref().map(|notes| (
                notes.version.as_str(),
                notes.body.as_str(),
                notes.preview
            )),
            Some(("0.8.3", "### Changed\n- Client shell", true))
        );
    }

    #[test]
    fn snapshot_badges_only_outdated_integrations() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.integration_recommendations =
            vec![crate::integration::IntegrationRecommendation {
                target: crate::api::schema::IntegrationTarget::Claude,
                label: "claude",
                command: "claude",
                available: true,
                path: std::path::PathBuf::from("claude-hook"),
                state: crate::integration::IntegrationStatusKind::NotInstalled,
            }];

        assert!(!snapshot(&app, "boot", 1, None, None).integration_updates_available);

        app.state.integration_recommendations[0].state =
            crate::integration::IntegrationStatusKind::Outdated;
        assert!(snapshot(&app, "boot", 2, None, None).integration_updates_available);
    }

    #[test]
    fn split_hits_follow_released_border_and_gap_geometry() {
        let horizontal = crate::layout::SplitBorder {
            pos: 20,
            direction: ratatui::layout::Direction::Horizontal,
            ratio: 0.5,
            area: Rect::new(2, 3, 40, 12),
            path: vec![false],
        };
        assert_eq!(
            split_hit_rect(&horizontal, true, false, &[]),
            Some(Rect::new(20, 3, 1, 12))
        );
        assert_eq!(
            split_hit_rect(&horizontal, true, true, &[]),
            Some(Rect::new(19, 3, 2, 12))
        );
        assert_eq!(
            split_hit_rect(&horizontal, false, true, &[]),
            Some(Rect::new(19, 3, 1, 12))
        );
        assert_eq!(split_hit_rect(&horizontal, false, false, &[]), None);

        let vertical = crate::layout::SplitBorder {
            pos: 9,
            direction: ratatui::layout::Direction::Vertical,
            ratio: 0.5,
            area: Rect::new(2, 3, 40, 12),
            path: vec![true],
        };
        assert_eq!(
            split_hit_rect(&vertical, true, true, &[]),
            Some(Rect::new(2, 8, 40, 2))
        );

        let edge = crate::layout::SplitBorder {
            pos: 0,
            direction: ratatui::layout::Direction::Horizontal,
            ratio: 0.5,
            area: Rect::new(0, 0, 1, 4),
            path: Vec::new(),
        };
        assert_eq!(
            split_hit_rect(&edge, true, true, &[]),
            Some(Rect::new(0, 0, 1, 4))
        );
        assert_eq!(split_hit_rect(&edge, false, true, &[]), None);
        assert_eq!(
            split_hit_rect(&horizontal, false, true, &[Rect::new(19, 3, 1, 12)]),
            None
        );
    }
}
