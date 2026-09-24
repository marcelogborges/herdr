use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::cli::jira::ui::{BLUE, BUTTON_BG, GRAY, GREEN, ORANGE, PURPLE, RED, SELECT_BG};

use super::git::{FileChange, RepoDiff};
use super::state::{Action, DiffState, Hit, Row, View};
use super::worker::Source;

const TREE_ACTIONS: [Action; 6] = [
    Action::Open,
    Action::Mode,
    Action::Edit,
    Action::Lazygit,
    Action::Refresh,
    Action::Quit,
];
const FILE_ACTIONS: [Action; 6] = [
    Action::Back,
    Action::Next,
    Action::Previous,
    Action::Edit,
    Action::Lazygit,
    Action::Mode,
];

pub(crate) fn render(frame: &mut Frame, state: &mut DiffState) {
    state.hits.clear();
    let area = frame.area();
    if area.width < 10 || area.height < 4 {
        return;
    }
    let actions: &[Action] = match state.view {
        View::Tree => &TREE_ACTIONS,
        View::File(_) => &FILE_ACTIONS,
    };
    let buttons = layout_buttons(actions, area.width.saturating_sub(1));
    let button_rows = buttons
        .iter()
        .map(|(_, row, _)| *row)
        .max()
        .map_or(0, |row| row + 1);
    let footer = button_rows + 1;
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1 + footer),
    );
    state.list_height = body.height as usize;
    state.body_width = body.width;
    let names = display_names(&state.repos, state.home.as_deref());
    render_title(
        frame,
        state,
        &names,
        Rect::new(area.x, area.y, area.width, 1),
    );
    match state.view {
        View::Tree => render_tree(frame, state, &names, body),
        View::File(_) => render_file(frame, state, body),
    }
    render_notice(
        frame,
        state,
        Rect::new(area.x, area.y + area.height - footer, area.width, 1),
    );
    let top = area.y + area.height - button_rows;
    for (action, row, x) in buttons {
        let key = format!(" {}", action.key());
        let rest = format!(" {} ", action.name());
        let rect = Rect::new(
            area.x + x,
            top + row,
            (key.width() + rest.width()) as u16,
            1,
        );
        let line = Line::from(vec![
            Span::styled(
                key,
                Style::default()
                    .fg(BLUE)
                    .bg(BUTTON_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(rest, Style::default().bg(BUTTON_BG)),
        ]);
        frame.render_widget(Paragraph::new(line), rect);
        state.hits.push((rect, Hit::Button(action)));
    }
}

fn button_width(action: Action) -> u16 {
    (action.key().width() + action.name().width() + 3) as u16
}

pub(crate) fn layout_buttons(actions: &[Action], width: u16) -> Vec<(Action, u16, u16)> {
    let mut placed = Vec::new();
    let mut row = 0;
    let mut x = 0u16;
    for action in actions {
        let label_width = button_width(*action);
        if x > 0 && x + label_width > width {
            row += 1;
            x = 0;
        }
        placed.push((*action, row, x));
        x += label_width + 1;
    }
    placed
}

fn render_title(frame: &mut Frame, state: &DiffState, names: &[String], area: Rect) {
    let mut spans = vec![Span::styled(
        " diff",
        Style::default().fg(PURPLE).add_modifier(Modifier::BOLD),
    )];
    let focused = state
        .focused_root()
        .and_then(|root| state.repo_index(&root))
        .or_else(|| (!state.repos.is_empty()).then_some(0));
    match &state.view {
        View::File(view) => {
            let repo = state
                .repo_index(&view.root)
                .and_then(|index| names.get(index))
                .cloned()
                .unwrap_or_default();
            spans.push(Span::styled(
                format!(" · {repo} · "),
                Style::default().fg(GRAY),
            ));
            spans.push(Span::styled(
                view.path.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
        View::Tree => {
            let base = focused
                .and_then(|index| state.repos.get(index))
                .map(|repo| repo.base_label.clone())
                .filter(|label| !label.is_empty());
            let mode = match base {
                Some(base) => format!(" · {} vs {base}", state.mode.label()),
                None => format!(" · {}", state.mode.label()),
            };
            spans.push(Span::styled(mode, Style::default().fg(GRAY)));
        }
    }
    let status = if state.loading {
        Span::styled("atualizando…", Style::default().fg(ORANGE))
    } else if let View::File(view) = &state.view {
        let total = view.lines.as_ref().map_or(0, Vec::len);
        Span::styled(
            format!("{}/{total}", (view.scroll + 1).min(total)),
            Style::default().fg(GRAY),
        )
    } else {
        Span::raw("")
    };
    let used: usize = spans.iter().map(|span| span.content.width()).sum();
    let status_width = status.content.width();
    if used + status_width + 2 <= area.width as usize {
        spans.push(Span::raw(
            " ".repeat(area.width as usize - used - status_width - 1),
        ));
        spans.push(status);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_notice(frame: &mut Frame, state: &DiffState, area: Rect) {
    let Some(notice) = &state.notice else {
        return;
    };
    let style = Style::default().fg(if notice.error { RED } else { GREEN });
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate(&format!(" {}", notice.text), area.width as usize),
            style,
        ))),
        area,
    );
}

fn empty_message(state: &DiffState) -> Vec<Line<'static>> {
    if state.loading || state.source.is_none() {
        return vec![Line::from(Span::styled(
            " procurando repositórios…",
            Style::default().fg(GRAY),
        ))];
    }
    let mut lines = vec![
        Line::from(Span::styled(
            " nenhum repositório git por aqui",
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    let hint = if state.source == Some(Source::Nothing) {
        " a sessão Claude deste painel ainda não tocou em nenhum repo e o diretório atual não é um repositório git."
    } else {
        " nada encontrado."
    };
    lines.push(Line::from(Span::styled(hint, Style::default().fg(GRAY))));
    lines.push(Line::from(Span::styled(
        format!(" diretório: {}", state.cwd.display()),
        Style::default().fg(GRAY),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " r atualiza · q sai",
        Style::default().fg(GRAY),
    )));
    lines
}

fn render_tree(frame: &mut Frame, state: &mut DiffState, names: &[String], area: Rect) {
    let max_scroll = state.rows.len().saturating_sub(state.list_height.max(1));
    state.scroll = state.scroll.min(max_scroll);
    if state.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(empty_message(state)).wrap(ratatui::widgets::Wrap { trim: false }),
            area,
        );
        return;
    }
    let selected = state.selected_row();
    let width = area.width as usize;
    let mut hits = Vec::new();
    for (offset, index) in (state.scroll..state.rows.len())
        .take(area.height as usize)
        .enumerate()
    {
        let rect = Rect::new(area.x, area.y + offset as u16, area.width, 1);
        let row_style = if Some(index) == selected {
            Style::default().bg(SELECT_BG)
        } else {
            Style::default()
        };
        let line = match state.rows[index] {
            Row::Repo(repo) => {
                let owner = &state.repos[repo];
                let collapsed = state.is_collapsed(&owner.root);
                repo_line(owner, &names[repo], collapsed, width)
            }
            Row::File(repo, file) => {
                let Some(change) = state.file(repo, file) else {
                    continue;
                };
                file_line(change, width)
            }
        };
        frame.render_widget(Paragraph::new(line).style(row_style), rect);
        hits.push((rect, Hit::Row(index)));
    }
    state.hits.extend(hits);
}

fn repo_line(repo: &RepoDiff, name: &str, collapsed: bool, width: usize) -> Line<'static> {
    let changed = repo.has_changes();
    let mut right: Vec<Span<'static>> = if let Some(error) = &repo.error {
        vec![Span::styled(
            truncate(error, width / 2),
            Style::default().fg(RED),
        )]
    } else if changed {
        let count = repo.files.len();
        vec![
            Span::styled(format!("+{}", repo.adds()), Style::default().fg(GREEN)),
            Span::raw(" "),
            Span::styled(format!("−{}", repo.dels()), Style::default().fg(RED)),
            Span::styled(
                format!(" · {count} {}", if count == 1 { "arq" } else { "arqs" }),
                Style::default().fg(GRAY),
            ),
        ]
    } else {
        vec![Span::styled("sem mudanças", Style::default().fg(GRAY))]
    };
    right.push(Span::raw(" "));
    let right_width: usize = right.iter().map(|span| span.content.width()).sum();
    let marker = match (changed, collapsed) {
        (false, _) => "·",
        (true, true) => "▸",
        (true, false) => "▾",
    };
    let name_style = if changed {
        Style::default().fg(BLUE).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(GRAY)
    };
    let lead = format!(" {marker} ");
    let available = width.saturating_sub(lead.width() + right_width + 1);
    let name = truncate_left(name, available);
    let mut spans = vec![
        Span::styled(lead, name_style),
        Span::styled(name.clone(), name_style),
    ];
    let mut used = 3 + name.width();
    if let Some(branch) = &repo.branch {
        let room = width.saturating_sub(used + right_width + 3);
        if room >= 4 {
            let branch = truncate(branch, room);
            used += branch.width() + 2;
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                branch,
                Style::default().fg(if changed { PURPLE } else { GRAY }),
            ));
        }
    }
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(used + right_width)),
    ));
    spans.extend(right);
    Line::from(spans)
}

pub(crate) fn status_color(status: char) -> Color {
    match status {
        'A' => GREEN,
        'D' => RED,
        'R' | 'C' => PURPLE,
        '?' => BLUE,
        _ => ORANGE,
    }
}

fn file_line(change: &FileChange, width: usize) -> Line<'static> {
    let right: Vec<Span<'static>> = if change.binary {
        vec![Span::styled("bin ", Style::default().fg(GRAY))]
    } else {
        let mut spans = Vec::new();
        if change.adds > 0 {
            spans.push(Span::styled(
                format!("+{}", change.adds),
                Style::default().fg(GREEN),
            ));
        }
        if change.dels > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("−{}", change.dels),
                Style::default().fg(RED),
            ));
        }
        spans.push(Span::raw(" "));
        spans
    };
    let right_width: usize = right.iter().map(|span| span.content.width()).sum();
    let lead = "     ";
    let path = truncate_left(
        &change.path,
        width.saturating_sub(lead.width() + 2 + right_width + 1),
    );
    let used = lead.width() + 2 + path.width();
    let mut spans = vec![
        Span::raw(lead),
        Span::styled(
            change.status.to_string(),
            Style::default()
                .fg(status_color(change.status))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::raw(path),
        Span::raw(" ".repeat(width.saturating_sub(used + right_width))),
    ];
    spans.extend(right);
    Line::from(spans)
}

fn render_file(frame: &mut Frame, state: &mut DiffState, area: Rect) {
    let View::File(view) = &mut state.view else {
        return;
    };
    let message = match (&view.error, &view.lines) {
        (Some(error), _) => Some(Span::styled(format!(" {error}"), Style::default().fg(RED))),
        (None, None) => Some(Span::styled(" carregando diff…", Style::default().fg(GRAY))),
        (None, Some(lines)) if lines.is_empty() => Some(Span::styled(
            " diff vazio (mudança só de modo ou arquivo binário)",
            Style::default().fg(GRAY),
        )),
        _ => None,
    };
    if let Some(message) = message {
        frame.render_widget(Paragraph::new(Line::from(message)), area);
        return;
    }
    let lines = view.lines.as_deref().unwrap_or_default();
    let height = area.height as usize;
    view.scroll = view.scroll.min(lines.len().saturating_sub(height));
    let visible: Vec<Line<'static>> = lines
        .iter()
        .skip(view.scroll)
        .take(height)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(visible), area);
}

pub(crate) fn display_names(repos: &[RepoDiff], home: Option<&Path>) -> Vec<String> {
    let common = common_ancestor(repos.iter().map(|repo| repo.root.as_path()));
    let usable = common.filter(|common| {
        repos.len() > 1 && common.parent().is_some() && home.is_none_or(|home| common != home)
    });
    repos
        .iter()
        .map(|repo| {
            if let Some(common) = &usable {
                if let Ok(relative) = repo.root.strip_prefix(common) {
                    if !relative.as_os_str().is_empty() {
                        return relative.display().to_string();
                    }
                }
            }
            let workspace = home.map(|home| home.join("projects"));
            if let Some(relative) = workspace
                .as_deref()
                .and_then(|workspace| repo.root.strip_prefix(workspace).ok())
                .filter(|relative| !relative.as_os_str().is_empty())
            {
                return relative.display().to_string();
            }
            match home.and_then(|home| repo.root.strip_prefix(home).ok()) {
                Some(relative) => format!("~/{}", relative.display()),
                None => repo.root.display().to_string(),
            }
        })
        .collect()
}

fn common_ancestor<'a>(mut paths: impl Iterator<Item = &'a Path>) -> Option<PathBuf> {
    let mut common = paths.next()?.parent()?.to_path_buf();
    for path in paths {
        while !path.starts_with(&common) {
            if !common.pop() {
                return None;
            }
        }
    }
    Some(common)
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}

fn truncate_left(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > width {
            break;
        }
        kept.push(ch);
        used += ch_width;
    }
    kept.push('…');
    kept.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::diff::state::tests::{loaded_state, press};
    use crate::cli::diff::worker::{Job, Outcome};
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(state: &mut DiffState, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, state)).unwrap();
        terminal
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn click(state: &mut DiffState, column: u16, row: u16) -> Vec<Job> {
        state
            .mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
            .jobs
    }

    #[test]
    fn tree_shows_repo_headers_files_and_mode() {
        let mut state = loaded_state();
        let text = screen(&draw(&mut state, 60, 12));

        assert!(text.contains("diff · PR vs origin/develop"), "{text}");
        assert!(text.contains("▾ api  task/x"), "{text}");
        assert!(text.contains("+12 −1 · 2 arqs"), "{text}");
        assert!(text.contains("M app/a.rb"), "{text}");
        assert!(text.contains("▸ web"), "{text}");
        assert!(text.contains("· clean"), "{text}");
        assert!(text.contains("sem mudanças"), "{text}");
        assert!(text.contains("b modo"), "{text}");
    }

    #[test]
    fn clicks_toggle_repos_open_files_on_double_click_and_press_buttons() {
        let mut state = loaded_state();
        draw(&mut state, 60, 12);
        assert!(click(&mut state, 3, 4).is_empty());
        assert!(!state.is_collapsed(Path::new("/w/web")));

        draw(&mut state, 60, 12);
        assert!(click(&mut state, 10, 3).is_empty());
        let jobs = click(&mut state, 10, 3);
        assert!(
            matches!(jobs.as_slice(), [Job::FileDiff { file, .. }] if file.path == "spec/a_spec.rb")
        );

        state.apply(Outcome::FileDiff {
            root: "/w/api".into(),
            path: "spec/a_spec.rb".into(),
            width: 60,
            result: Ok(crate::cli::diff::ansi::parse(
                b"\x1b[32m+ added line\x1b[0m\n",
            )),
        });
        let terminal = draw(&mut state, 60, 12);
        let text = screen(&terminal);
        assert!(text.contains("api · spec/a_spec.rb"), "{text}");
        assert!(text.contains("+ added line"), "{text}");
        let back = state
            .hits
            .iter()
            .find(|(_, hit)| *hit == Hit::Button(Action::Back))
            .map(|(rect, _)| *rect)
            .unwrap();
        click(&mut state, back.x + 1, back.y);
        assert_eq!(state.view, View::Tree);
    }

    #[test]
    fn empty_state_explains_what_was_searched() {
        let mut state = DiffState::new("/home/me/projects".into(), Some("/home/me".into()));
        state.start_refresh();
        state.apply(Outcome::Loaded {
            mode: state.mode,
            repos: vec![],
            source: Source::Nothing,
        });
        let text = screen(&draw(&mut state, 60, 12));
        assert!(text.contains("nenhum repositório git por aqui"), "{text}");
        assert!(text.contains("/home/me/projects"), "{text}");
        assert!(state.key(press(KeyCode::Char('q'))).quit);
    }

    #[test]
    fn names_strip_the_shared_workspace_or_fall_back_to_home() {
        let repo = |root: &str| RepoDiff {
            root: root.into(),
            ..RepoDiff::default()
        };
        let home = Path::new("/home/me");
        assert_eq!(
            display_names(
                &[
                    repo("/home/me/p/api-worktrees/VK-1"),
                    repo("/home/me/p/web")
                ],
                Some(home)
            ),
            ["api-worktrees/VK-1", "web"]
        );
        assert_eq!(
            display_names(&[repo("/home/me/p/api")], Some(home)),
            ["~/p/api"]
        );
        assert_eq!(
            display_names(&[repo("/home/me/projects/vakinha-api")], Some(home)),
            ["vakinha-api"]
        );
        assert_eq!(
            display_names(&[repo("/home/me/a"), repo("/opt/b")], Some(home)),
            ["~/a", "/opt/b"]
        );
        assert_eq!(truncate_left("src/deep/file.rb", 8), "…file.rb");
    }
}
