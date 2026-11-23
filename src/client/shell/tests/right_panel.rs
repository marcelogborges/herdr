use super::*;
use crate::input::{TerminalKey, TextCommit};

const PANEL_ID: &str = "right-panel:term_panel";

fn panel_pane() -> PaneSurfacePane {
    PaneSurfacePane {
        pane_id: PANEL_ID.into(),
        content_revision: 1,
        rect: SurfaceRect {
            x: 10,
            y: 0,
            width: 30,
            height: 10,
        },
        inner_rect: SurfaceRect {
            x: 11,
            y: 1,
            width: 29,
            height: 9,
        },
        scrollbar_rect: None,
        scroll: None,
        focused: false,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn state_with_panel() -> ClientShellState {
    state_with_panel_tabs(Vec::new())
}

fn state_with_panel_tabs(right_panel_tabs: Vec<String>) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut snapshot = snapshot();
    snapshot.right_panel_tabs = right_panel_tabs;
    state.set_snapshot(Box::new(snapshot));
    let mut surface = surface();
    surface.panes.push(panel_pane());
    state.set_pane_surface(surface);
    state.compose(106, 20).expect("pane frame");
    state
}

fn panel_hit(state: &ClientShellState) -> PaneHit {
    state
        .hits
        .panes
        .iter()
        .find(|hit| hit.pane_id == PANEL_ID)
        .cloned()
        .expect("panel hit")
}

fn click(state: &mut ClientShellState, column: u16, row: u16) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

fn endpoint_methods(input: &ClientShellInput) -> Vec<crate::api::schema::Method> {
    input
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(request.method.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn header_tab_click_switches_the_panel_mode() {
    let mut state = state_with_panel_tabs(vec!["rc".into()]);
    let hit = panel_hit(&state);
    let modes = crate::right_panel::header_modes(&["rc".into()]);
    let (_, files) = crate::right_panel::header_tabs(hit.rect, &modes)
        .into_iter()
        .find(|(mode, _)| *mode == crate::right_panel::RightPanelMode::Files)
        .expect("files tab");

    let input = click(&mut state, files.x + 1, files.y);

    assert!(matches!(
        &endpoint_methods(&input)[..],
        [crate::api::schema::Method::RightPanelShow(params)]
            if params.mode == crate::right_panel::RightPanelMode::Files
    ));
}

#[test]
fn header_click_on_a_custom_tab_shows_it_by_label() {
    let mut state = state_with_panel_tabs(vec!["rc".into()]);
    let hit = panel_hit(&state);
    let custom = crate::right_panel::RightPanelMode::Custom("rc".into());
    let modes = crate::right_panel::header_modes(&["rc".into()]);
    let (_, rect) = crate::right_panel::header_tabs(hit.rect, &modes)
        .into_iter()
        .find(|(mode, _)| *mode == custom)
        .expect("custom tab");

    let input = click(&mut state, rect.x + 1, rect.y);

    assert!(matches!(
        &endpoint_methods(&input)[..],
        [crate::api::schema::Method::RightPanelShow(params)] if params.mode == custom
    ));
}

#[test]
fn clicking_panel_content_focuses_the_panel_pseudo_pane() {
    let mut state = state_with_panel();
    let hit = panel_hit(&state);

    let input = click(&mut state, hit.inner_rect.x + 2, hit.inner_rect.y + 2);

    assert!(endpoint_methods(&input).iter().any(|method| matches!(
        method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == PANEL_ID
    )));
}

#[test]
fn prefix_i_toggles_the_right_panel() {
    let mut state = state_with_panel();

    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Char('b'),
        KeyModifiers::CONTROL,
    ))]);
    let input = state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Char('i'),
        KeyModifiers::NONE,
    ))]);

    assert!(matches!(
        &endpoint_methods(&input)[..],
        [crate::api::schema::Method::RightPanelToggle(_)]
    ));
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

fn sent_cycle_directions(
    input: &ClientShellInput,
) -> Vec<crate::right_panel::RightPanelCycleDirection> {
    endpoint_methods(input)
        .into_iter()
        .filter_map(|method| match method {
            crate::api::schema::Method::RightPanelCycle(params) => Some(params.direction),
            _ => None,
        })
        .collect()
}

#[test]
fn alt_q_and_alt_e_cycle_the_right_panel_mode() {
    let mut state = state_with_panel();

    let next = state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Char('q'),
        KeyModifiers::ALT,
    ))]);
    let previous = state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Char('e'),
        KeyModifiers::ALT,
    ))]);

    assert_eq!(
        sent_cycle_directions(&next),
        vec![crate::right_panel::RightPanelCycleDirection::Next]
    );
    assert_eq!(
        sent_cycle_directions(&previous),
        vec![crate::right_panel::RightPanelCycleDirection::Previous]
    );
}

#[test]
fn alt_q_cycles_instead_of_typing_when_the_panel_has_focus() {
    let mut state = state_with_panel();
    let mut focused = snapshot();
    focused.focused_pane_id = Some(PANEL_ID.into());
    state.set_snapshot(Box::new(focused));

    let input = state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Char('q'),
        KeyModifiers::ALT,
    ))]);

    assert_eq!(
        sent_cycle_directions(&input),
        vec![crate::right_panel::RightPanelCycleDirection::Next]
    );
    assert!(!input
        .requests
        .iter()
        .any(|request| matches!(request, ClientMessage::ClientShellPaneInput { .. })));
}

#[test]
fn typing_goes_to_the_panel_when_the_snapshot_focuses_it() {
    let mut state = state_with_panel();
    let mut focused = snapshot();
    focused.focused_pane_id = Some(PANEL_ID.into());
    state.set_snapshot(Box::new(focused));

    let input = state.handle_raw_events(vec![RawInputEvent::Text(TextCommit::new("q"))]);

    assert!(matches!(
        &input.requests[..],
        [ClientMessage::ClientShellPaneInput { pane_id, .. }] if pane_id == PANEL_ID
    ));
}

fn mouse(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

fn sent_widths(input: &ClientShellInput) -> Vec<Option<u16>> {
    endpoint_methods(input)
        .into_iter()
        .filter_map(|method| match method {
            crate::api::schema::Method::RightPanelSetWidth(params) => Some(params.width),
            _ => None,
        })
        .collect()
}

#[test]
fn dragging_the_panel_divider_sends_throttled_widths_and_a_final_width() {
    let mut state = state_with_panel();
    let hit = panel_hit(&state);
    let right_edge = hit.rect.x + hit.rect.width;
    let row = hit.rect.y + 2;

    let press = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        hit.rect.x,
        row,
    );
    assert!(endpoint_methods(&press).is_empty());
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::RightPanelWidth { .. })
    ));

    let first = mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        hit.rect.x - 5,
        row,
    );
    assert_eq!(sent_widths(&first), [Some(right_edge - (hit.rect.x - 5))]);

    let throttled = mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        hit.rect.x - 6,
        row,
    );
    assert!(sent_widths(&throttled).is_empty());

    let release = mouse(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        hit.rect.x - 8,
        row,
    );
    assert_eq!(sent_widths(&release), [Some(right_edge - (hit.rect.x - 8))]);
    assert!(state.chrome_drag.is_none());
}

#[test]
fn releasing_at_the_last_sent_width_sends_nothing_more() {
    let mut state = state_with_panel();
    let hit = panel_hit(&state);
    let row = hit.rect.y + 2;
    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        hit.rect.x,
        row,
    );
    mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        hit.rect.x + 3,
        row,
    );

    let release = mouse(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        hit.rect.x + 3,
        row,
    );

    assert!(sent_widths(&release).is_empty());
}

#[test]
fn double_clicking_the_panel_divider_resets_the_width() {
    let mut state = state_with_panel();
    let hit = panel_hit(&state);
    let row = hit.rect.y + 2;
    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        hit.rect.x,
        row,
    );
    mouse(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        hit.rect.x,
        row,
    );

    let second = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        hit.rect.x,
        row,
    );

    assert_eq!(sent_widths(&second), [None]);
    assert!(state.chrome_drag.is_none());
}

#[test]
fn pressing_inside_the_panel_does_not_start_a_width_drag() {
    let mut state = state_with_panel();
    let hit = panel_hit(&state);

    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        hit.rect.x + 1,
        hit.rect.y + 2,
    );

    assert!(!matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::RightPanelWidth { .. })
    ));
}

fn state_with_reporting_panel(sgr_pixel_mouse: bool) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut surface = surface();
    let mut panel = panel_pane();
    panel.mouse_reporting = true;
    panel.sgr_pixel_mouse = sgr_pixel_mouse;
    panel.pixel_width = 290;
    panel.pixel_height = 180;
    surface.panes.push(panel);
    state.set_pane_surface(surface);
    state.compose(106, 20).expect("pane frame");
    state
}

fn panel_mouse_inputs(
    input: &ClientShellInput,
) -> Vec<(crate::protocol::ClientMouseKind, ClientMousePosition)> {
    input
        .requests
        .iter()
        .filter_map(|request| match request {
            ClientMessage::ClientShellPaneInput { pane_id, events } if pane_id == PANEL_ID => {
                Some(events)
            }
            _ => None,
        })
        .flatten()
        .filter_map(|event| match event {
            ClientPaneInputEvent::Mouse { kind, position, .. } => Some((*kind, *position)),
            _ => None,
        })
        .collect()
}

#[test]
fn reporting_panel_receives_the_whole_drag_in_content_cells_without_a_selection() {
    let mut state = state_with_reporting_panel(false);
    let hit = panel_hit(&state);
    let (x, y) = (hit.inner_rect.x, hit.inner_rect.y);
    let left = crate::protocol::ClientMouseButton::Left;

    let press = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        x + 2,
        y + 2,
    );
    let drag = mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        x + 4,
        y + 3,
    );
    let outside = mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        x - 5,
        y + 3,
    );
    let release = mouse(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        x - 6,
        y + 3,
    );

    assert_eq!(
        panel_mouse_inputs(&press),
        [(
            crate::protocol::ClientMouseKind::Down(left),
            ClientMousePosition::Cell { column: 2, row: 2 }
        )]
    );
    assert_eq!(
        panel_mouse_inputs(&drag),
        [(
            crate::protocol::ClientMouseKind::Drag(left),
            ClientMousePosition::Cell { column: 4, row: 3 }
        )]
    );
    assert_eq!(
        panel_mouse_inputs(&outside),
        [(
            crate::protocol::ClientMouseKind::Drag(left),
            ClientMousePosition::Cell { column: 0, row: 3 }
        )]
    );
    assert_eq!(
        panel_mouse_inputs(&release),
        [(
            crate::protocol::ClientMouseKind::Up(left),
            ClientMousePosition::Cell { column: 0, row: 3 }
        )]
    );
    assert!(state.selection.is_none());
    assert!(state.pane_mouse_gesture.is_none());
}

#[test]
fn reporting_panel_double_click_reaches_the_program_instead_of_word_selection() {
    let mut state = state_with_reporting_panel(false);
    let hit = panel_hit(&state);
    let (x, y) = (hit.inner_rect.x + 3, hit.inner_rect.y + 1);

    let mut inputs = Vec::new();
    for _ in 0..2 {
        inputs.extend(panel_mouse_inputs(&mouse(
            &mut state,
            MouseEventKind::Down(MouseButton::Left),
            x,
            y,
        )));
        inputs.extend(panel_mouse_inputs(&mouse(
            &mut state,
            MouseEventKind::Up(MouseButton::Left),
            x,
            y,
        )));
    }

    assert_eq!(inputs.len(), 4);
    assert!(inputs
        .iter()
        .all(|(_, position)| *position == ClientMousePosition::Cell { column: 3, row: 1 }));
    assert!(state.selection.is_none());
    assert!(state.last_pane_click.is_none());
}

#[test]
fn reporting_panel_maps_host_pixels_into_its_content_and_pins_a_release_outside_it() {
    let mut state = state_with_reporting_panel(true);
    let hit = panel_hit(&state);
    let geometry =
        crate::input::mouse::HostGeometry::new(106, 20, 1060, 400).expect("host geometry");
    let x = u32::from(hit.inner_rect.x + 2) * 10 + 6;
    let y = u32::from(hit.inner_rect.y + 2) * 20 + 11;

    let press = state.handle_pixel_mouse(format!("\x1b[<0;{x};{y}M").as_bytes(), geometry);

    assert_eq!(
        panel_mouse_inputs(&press),
        [(
            crate::protocol::ClientMouseKind::Down(crate::protocol::ClientMouseButton::Left),
            ClientMousePosition::Pixels {
                x: 26,
                y: 51,
                column: 2,
                row: 2
            }
        )]
    );

    let outside = format!("\x1b[<0;41;{y}m");
    let release = state.handle_pixel_mouse(outside.as_bytes(), geometry);

    assert_eq!(
        panel_mouse_inputs(&release),
        [(
            crate::protocol::ClientMouseKind::Up(crate::protocol::ClientMouseButton::Left),
            ClientMousePosition::Pixels {
                x: 1,
                y: 51,
                column: 0,
                row: 2
            }
        )]
    );
    assert!(state.pane_mouse_gesture.is_none());
}
