use std::collections::HashSet;
use std::time::Instant;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::adf::{self, RichLine, RichSpan, SpanStyle, WrappedSpan};
use super::model::{priority_icon, IssueDetail, StatusCategory};
use super::state::{Action, DetailSection, Hit, JiraState, Popup, Row, View};

pub(crate) const BLUE: Color = Color::Rgb(0x2e, 0x7d, 0xe9);
pub(crate) const PURPLE: Color = Color::Rgb(0x98, 0x54, 0xf1);
pub(crate) const ORANGE: Color = Color::Rgb(0xdf, 0x8e, 0x1d);
pub(crate) const GREEN: Color = Color::Rgb(0x58, 0x75, 0x39);
pub(crate) const GRAY: Color = Color::Rgb(0x6c, 0x6f, 0x85);
pub(crate) const RED: Color = Color::Rgb(0xd2, 0x0f, 0x39);
pub(crate) const SELECT_BG: Color = Color::Rgb(0xdc, 0xe0, 0xe8);
pub(crate) const BUTTON_BG: Color = Color::Rgb(0xe6, 0xe9, 0xef);

const LIST_ACTIONS: [Action; 6] = [
    Action::Move,
    Action::Comment,
    Action::AssignMe,
    Action::Browser,
    Action::Session,
    Action::Refresh,
];
const DETAIL_ACTIONS: [Action; 7] = [
    Action::Back,
    Action::Move,
    Action::Comment,
    Action::AssignMe,
    Action::Browser,
    Action::Session,
    Action::Refresh,
];

pub(crate) fn render(frame: &mut Frame, state: &mut JiraState, now: Instant) {
    state.hits.clear();
    let area = frame.area();
    if area.width < 10 || area.height < 4 {
        return;
    }
    let actions: &[Action] = match state.view {
        View::List => &LIST_ACTIONS,
        View::Detail { .. } => &DETAIL_ACTIONS,
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

    render_title(frame, state, Rect::new(area.x, area.y, area.width, 1), now);
    match &state.view {
        View::List => render_list(frame, state, body),
        View::Detail { .. } => render_detail(frame, state, body, now),
    }
    render_notice(
        frame,
        state,
        Rect::new(area.x, area.y + area.height - footer, area.width, 1),
    );
    let top = area.y + area.height - button_rows;
    for (action, row, x) in buttons {
        let label = format!(" {} ", action.label());
        let rect = Rect::new(area.x + x, top + row, label.width() as u16, 1);
        let (key, rest) = label.split_at(2);
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
    match state.popup.clone() {
        Some(Popup::Transitions {
            key,
            transitions,
            error,
            selected,
        }) => render_transitions(
            frame,
            state,
            area,
            &key,
            transitions.as_deref(),
            error.as_deref(),
            selected,
        ),
        Some(Popup::Comment {
            key,
            text,
            cursor,
            sending,
        }) => render_comment(frame, state, area, &key, &text, cursor, sending),
        None => {}
    }
}

pub(crate) fn layout_buttons(actions: &[Action], width: u16) -> Vec<(Action, u16, u16)> {
    let mut placed = Vec::new();
    let mut row = 0;
    let mut x = 0u16;
    for action in actions {
        let label_width = action.label().width() as u16 + 2;
        if x > 0 && x + label_width > width {
            row += 1;
            x = 0;
        }
        placed.push((*action, row, x));
        x += label_width + 1;
    }
    placed
}

fn render_title(frame: &mut Frame, state: &JiraState, area: Rect, now: Instant) {
    let count = state.issues.len();
    let mut spans = vec![
        Span::styled(
            " jira",
            Style::default().fg(PURPLE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {} · {count} tasks", state.project),
            Style::default().fg(GRAY),
        ),
    ];
    let status = if state.loading {
        Span::styled("atualizando…", Style::default().fg(ORANGE))
    } else if state.list_error.is_some() {
        Span::styled("erro ao atualizar", Style::default().fg(RED))
    } else if let Some(last) = state.last_refresh {
        Span::styled(
            format!(
                "atualizado {}",
                elapsed_label(now.duration_since(last).as_secs())
            ),
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

fn render_notice(frame: &mut Frame, state: &JiraState, area: Rect) {
    let (text, error) = match (&state.notice, &state.list_error) {
        (Some(notice), _) => (notice.text.clone(), notice.error),
        (None, Some(error)) => (error.clone(), true),
        (None, None) => return,
    };
    let style = Style::default().fg(if error { RED } else { GREEN });
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate(&format!(" {text}"), area.width as usize),
            style,
        ))),
        area,
    );
}

fn render_list(frame: &mut Frame, state: &mut JiraState, area: Rect) {
    state.list_height = area.height as usize;
    let max_scroll = state.rows.len().saturating_sub(state.list_height.max(1));
    state.scroll = state.scroll.min(max_scroll);
    if state.rows.is_empty() {
        let text = if state.loading {
            "carregando tasks…".to_owned()
        } else if let Some(error) = &state.list_error {
            error.clone()
        } else {
            "nenhuma task aberta".to_owned()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {text}"),
                Style::default().fg(GRAY),
            ))),
            area,
        );
        return;
    }
    let selected = state.selected_row();
    let width = area.width as usize;
    let mut row_hits = Vec::new();
    let mut dot_hits = Vec::new();
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
        let line = match &state.rows[index] {
            Row::Header {
                status,
                count,
                collapsed,
            } => {
                let category = state
                    .issues
                    .iter()
                    .find(|issue| issue.status == *status)
                    .map(|issue| issue.category);
                let color = match category {
                    Some(StatusCategory::New) => GRAY,
                    Some(StatusCategory::Done) => GREEN,
                    _ => BLUE,
                };
                let marker = if *collapsed { "▸" } else { "▾" };
                Line::from(vec![
                    Span::styled(
                        format!(" {marker} {status}"),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" ({count})"), Style::default().fg(GRAY)),
                ])
            }
            Row::Issue(key) => {
                let Some(issue) = state.issue(key) else {
                    continue;
                };
                let (dot, dot_style) = match state.links.get(key) {
                    Some(link) if link.pane_id.is_some() => (
                        "●",
                        Style::default().fg(agent_color(link.agent_status.as_deref())),
                    ),
                    Some(_) => ("○", Style::default().fg(GRAY)),
                    None => (" ", Style::default()),
                };
                if dot != " " {
                    dot_hits.push((Rect::new(area.x + 1, rect.y, 2, 1), Hit::Dot(key.clone())));
                }
                let priority = priority_icon(issue.priority.as_deref());
                let priority_style = match priority {
                    "⇈" => Style::default().fg(RED),
                    "↑" => Style::default().fg(ORANGE),
                    "↓" | "⇊" => Style::default().fg(GRAY),
                    _ => Style::default().fg(GRAY),
                };
                let lead = format!("   {} {} ", key, priority);
                let summary = truncate(&issue.summary, width.saturating_sub(lead.width() + 1));
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(dot, dot_style),
                    Span::raw(" "),
                    Span::styled(
                        key.clone(),
                        Style::default().fg(PURPLE).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(priority, priority_style),
                    Span::raw(" "),
                    Span::raw(summary),
                ])
            }
        };
        frame.render_widget(Paragraph::new(line).style(row_style), rect);
        row_hits.push((rect, Hit::Row(index)));
    }
    state.hits.extend(row_hits);
    state.hits.extend(dot_hits);
}

fn agent_color(status: Option<&str>) -> Color {
    match status {
        Some("working") => ORANGE,
        Some("blocked") => RED,
        Some("idle") | Some("done") => GREEN,
        _ => BLUE,
    }
}

fn render_detail(frame: &mut Frame, state: &mut JiraState, area: Rect, now: Instant) {
    let View::Detail {
        key,
        detail,
        error,
        scroll,
    } = &state.view
    else {
        return;
    };
    let lines = detail_lines(
        key,
        detail.as_deref(),
        error.as_deref(),
        &state.browse_base,
        &state.collapsed_sections,
        now,
    );
    let wrapped = adf::wrap(&lines, area.width.saturating_sub(2) as usize);
    let height = area.height as usize;
    let max_scroll = wrapped.len().saturating_sub(height);
    let scroll = (*scroll).min(max_scroll);
    if let View::Detail { scroll: slot, .. } = &mut state.view {
        *slot = scroll;
    }
    state.detail_lines = wrapped.len();
    state.detail_height = height;
    for (offset, row) in wrapped.iter().skip(scroll).take(height).enumerate() {
        let y = area.y + offset as u16;
        let mut x = area.x + 1;
        let spans: Vec<Span> = row
            .iter()
            .map(|span| {
                let width = span.text.width() as u16;
                if let Some(url) = &span.link {
                    state.hits.push((Rect::new(x, y, width, 1), link_hit(url)));
                }
                x += width;
                Span::styled(span.text.clone(), span_style(span))
            })
            .collect();
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x + 1, y, area.width.saturating_sub(1), 1),
        );
    }
}

const SECTION_LINK: &str = "herdr-jira:section:";
const WORKTREE_LINK: &str = "herdr-jira:worktree:";

fn section_link(section: DetailSection) -> String {
    let name = match section {
        DetailSection::PullRequests => "pull-requests",
        DetailSection::Worktrees => "worktrees",
    };
    format!("{SECTION_LINK}{name}")
}

pub(crate) fn link_hit(link: &str) -> Hit {
    if let Some(name) = link.strip_prefix(SECTION_LINK) {
        match name {
            "pull-requests" => return Hit::Section(DetailSection::PullRequests),
            "worktrees" => return Hit::Section(DetailSection::Worktrees),
            _ => {}
        }
    }
    if let Some(index) = link
        .strip_prefix(WORKTREE_LINK)
        .and_then(|index| index.parse().ok())
    {
        return Hit::Worktree(index);
    }
    Hit::Link(link.to_owned())
}

fn section_header(
    lines: &mut Vec<RichLine>,
    section: DetailSection,
    title: &str,
    count: usize,
    collapsed: &HashSet<DetailSection>,
) -> bool {
    let open = !collapsed.contains(&section);
    let marker = if open { "▾" } else { "▸" };
    let hint = match section {
        DetailSection::PullRequests => "  p",
        DetailSection::Worktrees => "  t",
    };
    lines.push(RichLine {
        spans: vec![
            RichSpan {
                text: format!("{marker} {title} ({count})"),
                style: SpanStyle {
                    bold: count > 0,
                    dim: count == 0,
                    ..SpanStyle::default()
                },
                link: Some(section_link(section)),
            },
            span(
                hint,
                SpanStyle {
                    dim: true,
                    ..SpanStyle::default()
                },
            ),
        ],
        ..RichLine::default()
    });
    if open && count == 0 {
        lines.push(RichLine {
            indent: 2,
            spans: vec![span(
                "nenhum",
                SpanStyle {
                    dim: true,
                    ..SpanStyle::default()
                },
            )],
            ..RichLine::default()
        });
    }
    open
}

pub(crate) fn detail_lines(
    key: &str,
    detail: Option<&IssueDetail>,
    error: Option<&str>,
    browse_base: &str,
    collapsed: &HashSet<DetailSection>,
    now: Instant,
) -> Vec<RichLine> {
    let _ = now;
    let bold = SpanStyle {
        bold: true,
        ..SpanStyle::default()
    };
    let dim = SpanStyle {
        dim: true,
        ..SpanStyle::default()
    };
    let heading = SpanStyle {
        heading: true,
        bold: true,
        ..SpanStyle::default()
    };
    let mut lines = Vec::new();
    let Some(detail) = detail else {
        lines.push(RichLine::plain(key, heading));
        lines.push(RichLine::plain(error.unwrap_or("carregando…"), dim));
        return lines;
    };
    let issue = &detail.issue;
    lines.push(RichLine {
        spans: vec![
            span(
                &issue.key,
                SpanStyle {
                    mention: true,
                    bold: true,
                    ..SpanStyle::default()
                },
            ),
            span(&format!(" · {} · ", issue.issue_type), dim),
            span(
                &issue.status,
                SpanStyle {
                    code: false,
                    bold: true,
                    ..SpanStyle::default()
                },
            ),
        ],
        ..RichLine::default()
    });
    lines.push(RichLine::plain(&issue.summary, bold));
    lines.push(RichLine::default());
    let mut field = |label: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            lines.push(RichLine {
                spans: vec![
                    span(&format!("{label}: "), dim),
                    span(&value, SpanStyle::default()),
                ],
                ..RichLine::default()
            });
        }
    };
    field("prioridade", issue.priority.clone());
    field(
        "responsável",
        Some(issue.assignee.clone().unwrap_or_else(|| "ninguém".into())),
    );
    field("sprint", detail.sprint.clone());
    field(
        "pontos",
        detail.story_points.map(|points| {
            if points.fract() == 0.0 {
                format!("{points:.0}")
            } else {
                format!("{points}")
            }
        }),
    );
    field(
        "pai",
        issue
            .parent
            .as_ref()
            .map(|(key, summary)| format!("{key} {summary}")),
    );
    field("desenvolvimento", detail.development.clone());
    let status_width = detail
        .pull_requests
        .iter()
        .map(|pull_request| pull_request.status.chars().count())
        .max()
        .unwrap_or(0);
    if section_header(
        &mut lines,
        DetailSection::PullRequests,
        "pull requests",
        detail.pull_requests.len(),
        collapsed,
    ) {
        for pull_request in &detail.pull_requests {
            let status = pull_request.status.to_lowercase();
            let declined = matches!(status.as_str(), "declined" | "closed");
            let status_style = SpanStyle {
                bold: status == "open",
                dim: status != "open",
                ..SpanStyle::default()
            };
            lines.push(RichLine {
                indent: 2,
                spans: vec![
                    span(&format!("{status:<status_width$}  "), status_style),
                    RichSpan {
                        text: pull_request.label(),
                        style: SpanStyle {
                            underline: true,
                            strike: declined,
                            dim: declined,
                            ..SpanStyle::default()
                        },
                        link: Some(pull_request.url.clone()),
                    },
                ],
                ..RichLine::default()
            });
            let title = pull_request.title.trim();
            if !title.is_empty() && !title.starts_with(&issue.key) {
                lines.push(RichLine {
                    indent: 4 + status_width,
                    spans: vec![span(title, dim)],
                    ..RichLine::default()
                });
            }
        }
    }
    let repo_width = detail
        .worktrees
        .iter()
        .map(|worktree| worktree.repo.chars().count())
        .max()
        .unwrap_or(0);
    if section_header(
        &mut lines,
        DetailSection::Worktrees,
        "worktrees",
        detail.worktrees.len(),
        collapsed,
    ) {
        for (index, worktree) in detail.worktrees.iter().enumerate() {
            let mut spans = vec![
                span(&format!("{:<repo_width$}  ", worktree.repo), dim),
                RichSpan {
                    text: worktree.name.clone(),
                    style: SpanStyle {
                        underline: true,
                        ..SpanStyle::default()
                    },
                    link: Some(format!("{WORKTREE_LINK}{index}")),
                },
            ];
            if let Some(pull_request) = worktree.pull_request(&detail.pull_requests) {
                spans.push(span(
                    &format!(" → #{}", pull_request.number().unwrap_or("?")),
                    bold,
                ));
            }
            lines.push(RichLine {
                indent: 2,
                spans,
                ..RichLine::default()
            });
            if let Some(branch) = &worktree.branch {
                lines.push(RichLine {
                    indent: 4 + repo_width,
                    spans: vec![span(branch, dim)],
                    ..RichLine::default()
                });
            }
        }
    }
    let url = format!("{browse_base}{}", issue.key);
    lines.push(RichLine {
        spans: vec![
            span("link: ", dim),
            RichSpan {
                text: url.clone(),
                style: SpanStyle {
                    underline: true,
                    ..SpanStyle::default()
                },
                link: Some(url),
            },
        ],
        ..RichLine::default()
    });
    lines.push(RichLine::default());
    lines.push(RichLine::plain("Descrição", heading));
    match &detail.description {
        Some(description) => {
            let rendered = adf::render(description);
            if rendered.is_empty() {
                lines.push(RichLine::plain("(sem descrição)", dim));
            } else {
                lines.extend(rendered);
            }
        }
        None => lines.push(RichLine::plain("(sem descrição)", dim)),
    }
    lines.push(RichLine::default());
    lines.push(RichLine::plain(
        format!("Comentários ({})", detail.comments.len()),
        heading,
    ));
    if detail.comments.is_empty() {
        lines.push(RichLine::plain("(nenhum)", dim));
    }
    for comment in &detail.comments {
        lines.push(RichLine::default());
        lines.push(RichLine {
            spans: vec![
                span(
                    if comment.author.is_empty() {
                        "?"
                    } else {
                        &comment.author
                    },
                    bold,
                ),
                span(&format!(" · {}", relative_time(&comment.created)), dim),
            ],
            ..RichLine::default()
        });
        for mut line in adf::render(&comment.body) {
            line.indent += 2;
            lines.push(line);
        }
    }
    lines
}

fn span(text: &str, style: SpanStyle) -> RichSpan {
    RichSpan {
        text: text.to_owned(),
        style,
        link: None,
    }
}

fn span_style(span: &WrappedSpan) -> Style {
    let style_flags = span.style;
    let mut style = Style::default();
    if style_flags.heading {
        style = style.fg(BLUE);
    }
    if style_flags.code {
        style = style.fg(GREEN);
    }
    if style_flags.mention {
        style = style.fg(PURPLE);
    }
    if style_flags.quote || style_flags.dim {
        style = style.fg(GRAY);
    }
    if span.link.is_some() {
        style = style.fg(BLUE);
    }
    let mut modifiers = Modifier::empty();
    if style_flags.bold {
        modifiers |= Modifier::BOLD;
    }
    if style_flags.italic || style_flags.quote {
        modifiers |= Modifier::ITALIC;
    }
    if style_flags.underline {
        modifiers |= Modifier::UNDERLINED;
    }
    if style_flags.strike {
        modifiers |= Modifier::CROSSED_OUT;
    }
    style.add_modifier(modifiers)
}

fn popup_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(10);
    let height = height.min(area.height.saturating_sub(2)).max(3);
    Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    )
}

fn render_transitions(
    frame: &mut Frame,
    state: &mut JiraState,
    area: Rect,
    key: &str,
    transitions: Option<&[super::model::Transition]>,
    error: Option<&str>,
    selected: usize,
) {
    let items = transitions.map_or(1, |items| items.len().max(1)) as u16;
    let rect = popup_rect(area, 46, items + 2);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(PURPLE))
            .title(Span::styled(
                format!(" mover {key} "),
                Style::default().fg(PURPLE).add_modifier(Modifier::BOLD),
            )),
        rect,
    );
    state.hits.push((rect, Hit::PopupArea));
    let inner = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    );
    let placeholder = match (transitions, error) {
        (_, Some(error)) => Some((error.to_owned(), RED)),
        (None, None) => Some(("carregando transições…".to_owned(), GRAY)),
        (Some([]), None) => Some(("nenhuma transição disponível".to_owned(), GRAY)),
        _ => None,
    };
    if let Some((text, color)) = placeholder {
        frame.render_widget(
            Paragraph::new(Span::styled(
                truncate(&text, inner.width as usize),
                Style::default().fg(color),
            )),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return;
    }
    for (index, transition) in transitions
        .unwrap_or_default()
        .iter()
        .enumerate()
        .take(inner.height as usize)
    {
        let row = Rect::new(inner.x, inner.y + index as u16, inner.width, 1);
        let style = if index == selected {
            Style::default().bg(SELECT_BG)
        } else {
            Style::default()
        };
        let needs = if transition.required_fields.is_empty() {
            ""
        } else {
            " (browser)"
        };
        let label = if transition.name == transition.to {
            transition.to.clone()
        } else {
            format!("{} → {}", transition.name, transition.to)
        };
        let line = Line::from(vec![
            Span::styled(format!("{} ", index + 1), Style::default().fg(GRAY)),
            Span::styled(
                truncate(
                    &label,
                    (inner.width as usize).saturating_sub(3 + needs.len()),
                ),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(needs, Style::default().fg(GRAY)),
        ]);
        frame.render_widget(Paragraph::new(line).style(style), row);
        state.hits.push((row, Hit::MenuItem(index)));
    }
}

fn render_comment(
    frame: &mut Frame,
    state: &mut JiraState,
    area: Rect,
    key: &str,
    text: &str,
    cursor: usize,
    sending: bool,
) {
    let rect = popup_rect(area, area.width.saturating_sub(4).min(72), 12);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(PURPLE))
            .title(Span::styled(
                format!(" comentar {key} "),
                Style::default().fg(PURPLE).add_modifier(Modifier::BOLD),
            )),
        rect,
    );
    state.hits.push((rect, Hit::PopupArea));
    let inner = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(3),
    );
    let (rows, cursor_at) = wrap_editor(text, cursor, inner.width as usize);
    let first = cursor_at
        .0
        .saturating_sub(inner.height.saturating_sub(1) as usize);
    for (offset, row) in rows
        .iter()
        .skip(first)
        .take(inner.height as usize)
        .enumerate()
    {
        frame.render_widget(
            Paragraph::new(row.as_str()),
            Rect::new(inner.x, inner.y + offset as u16, inner.width, 1),
        );
    }
    if text.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "escreva o comentário… (enter = nova linha)",
                Style::default().fg(GRAY),
            )),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    if !sending {
        frame.set_cursor_position(Position::new(
            inner.x + cursor_at.1 as u16,
            inner.y + (cursor_at.0 - first) as u16,
        ));
    }
    let bottom = rect.y + rect.height - 2;
    let mut x = inner.x;
    if sending {
        frame.render_widget(
            Paragraph::new(Span::styled("enviando…", Style::default().fg(ORANGE))),
            Rect::new(x, bottom, inner.width, 1),
        );
        return;
    }
    for action in [Action::Send, Action::Cancel] {
        let label = format!(" {} ", action.label());
        let width = label.width() as u16;
        let button = Rect::new(x, bottom, width.min(inner.width), 1);
        frame.render_widget(
            Paragraph::new(Span::styled(
                label,
                Style::default()
                    .bg(BUTTON_BG)
                    .fg(if action == Action::Send { BLUE } else { GRAY })
                    .add_modifier(Modifier::BOLD),
            )),
            button,
        );
        state.hits.push((button, Hit::Button(action)));
        x += width + 1;
    }
}

pub(crate) fn wrap_editor(
    text: &str,
    cursor: usize,
    width: usize,
) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut cursor_at = (0, 0);
    let mut column = 0;
    for (index, ch) in text.char_indices() {
        if index == cursor {
            cursor_at = (rows.len() - 1, column);
        }
        if ch == '\n' {
            rows.push(String::new());
            column = 0;
            continue;
        }
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if column + ch_width > width {
            rows.push(String::new());
            column = 0;
        }
        rows.last_mut().unwrap().push(ch);
        column += ch_width;
    }
    if cursor >= text.len() {
        if column >= width {
            rows.push(String::new());
            column = 0;
        }
        cursor_at = (rows.len() - 1, column);
    }
    (rows, cursor_at)
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

fn elapsed_label(seconds: u64) -> String {
    match seconds {
        0..=9 => "agora".to_owned(),
        10..=59 => format!("há {seconds}s"),
        60..=3599 => format!("há {}m", seconds / 60),
        _ => format!("há {}h", seconds / 3600),
    }
}

pub(crate) fn relative_time(timestamp: &str) -> String {
    let Some(parsed) = parse_jira_time(timestamp) else {
        return timestamp.to_owned();
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let seconds = (now - parsed).max(0) as u64;
    match seconds {
        0..=59 => "agora".to_owned(),
        60..=3599 => format!("há {}m", seconds / 60),
        3600..=86_399 => format!("há {}h", seconds / 3600),
        86_400..=2_591_999 => format!("há {}d", seconds / 86_400),
        _ => timestamp.get(..10).unwrap_or(timestamp).to_owned(),
    }
}

pub(crate) fn parse_jira_time(timestamp: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| timestamp.get(range)?.parse::<i32>().ok();
    let year = number(0..4)?;
    let month = time::Month::try_from(number(5..7)? as u8).ok()?;
    let day = number(8..10)? as u8;
    let hour = number(11..13)? as u8;
    let minute = number(14..16)? as u8;
    let second = number(17..19)? as u8;
    let offset_start = timestamp[19..].find(['+', '-']).map(|index| index + 19)?;
    let sign = if &timestamp[offset_start..offset_start + 1] == "-" {
        -1
    } else {
        1
    };
    let offset_digits: String = timestamp[offset_start + 1..]
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let offset_hours = offset_digits.get(0..2)?.parse::<i8>().ok()?;
    let offset_minutes = offset_digits
        .get(2..4)
        .and_then(|value| value.parse::<i8>().ok())
        .unwrap_or(0);
    let offset = time::UtcOffset::from_hms(sign * offset_hours, sign * offset_minutes, 0).ok()?;
    let date = time::Date::from_calendar_date(year, month, day).ok()?;
    let clock = time::Time::from_hms(hour, minute, second).ok()?;
    Some(
        time::PrimitiveDateTime::new(date, clock)
            .assume_offset(offset)
            .unix_timestamp(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::jira::state::tests::{loaded_state, press};
    use crate::cli::jira::worker::{Job, Outcome};
    use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(state: &mut JiraState, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, state, Instant::now()))
            .unwrap();
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

    fn click(state: &mut JiraState, column: u16, row: u16) -> Vec<Job> {
        state
            .mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: crossterm::event::KeyModifiers::NONE,
            })
            .jobs
    }

    fn rect_of(state: &JiraState, wanted: &Hit) -> Rect {
        state
            .hits
            .iter()
            .find(|(_, hit)| hit == wanted)
            .map(|(rect, _)| *rect)
            .unwrap_or_else(|| panic!("no hit {wanted:?} in {:?}", state.hits))
    }

    #[test]
    fn list_renders_groups_dots_and_buttons() {
        let mut state = loaded_state();
        let terminal = draw(&mut state, 60, 14);
        let text = screen(&terminal);

        assert!(text.contains("jira · VK25 · 3 tasks"));
        assert!(text.contains("▾ Development (1)"));
        assert!(text.contains("● VK25-3 ↑ resumo VK25-3"));
        assert!(text.contains("m mover"));
        assert!(text.contains("w sessão"));
    }

    #[test]
    fn clicking_rows_buttons_dots_and_headers() {
        let mut state = loaded_state();
        draw(&mut state, 60, 14);

        let row = rect_of(&state, &Hit::Row(3));
        assert!(click(&mut state, row.x + 10, row.y).is_empty());
        assert_eq!(state.target_key().as_deref(), Some("VK25-1"));
        assert_eq!(
            click(&mut state, row.x + 10, row.y),
            [Job::Detail("VK25-1".into())]
        );

        state.key(press(KeyCode::Esc));
        draw(&mut state, 60, 14);
        let dot = rect_of(&state, &Hit::Dot("VK25-3".into()));
        assert!(
            matches!(click(&mut state, dot.x, dot.y).as_slice(), [Job::Session { key, .. }] if key == "VK25-3")
        );

        draw(&mut state, 60, 14);
        let refresh = rect_of(&state, &Hit::Button(Action::Refresh));
        assert_eq!(click(&mut state, refresh.x + 1, refresh.y), [Job::Refresh]);

        draw(&mut state, 60, 14);
        let header = rect_of(&state, &Hit::Row(0));
        click(&mut state, header.x + 2, header.y);
        assert!(state.rows.contains(&Row::Header {
            status: "Development".into(),
            count: 1,
            collapsed: true
        }));
    }

    #[test]
    fn move_menu_items_are_clickable_and_outside_click_cancels() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Char('m')));
        state.apply(Outcome::Transitions {
            key: "VK25-3".into(),
            transitions: vec![super::super::model::Transition {
                id: "7".into(),
                name: "Merge".into(),
                to: "QA - Staging".into(),
                required_fields: vec![],
            }],
        });
        let terminal = draw(&mut state, 60, 14);
        assert!(screen(&terminal).contains("1 Merge → QA - Staging"));

        let item = rect_of(&state, &Hit::MenuItem(0));
        let jobs = click(&mut state, item.x + 3, item.y);

        assert_eq!(
            jobs,
            [Job::Transition {
                key: "VK25-3".into(),
                transition_id: "7".into(),
                to: "QA - Staging".into()
            }]
        );

        state.key(press(KeyCode::Char('m')));
        draw(&mut state, 60, 14);
        click(&mut state, 0, 0);
        assert!(state.popup.is_none());
    }

    #[test]
    fn comment_popup_buttons_send_and_cancel() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Char('c')));
        state.paste("oi");
        let terminal = draw(&mut state, 60, 16);
        assert!(screen(&terminal).contains("comentar VK25-3"));

        let send = rect_of(&state, &Hit::Button(Action::Send));
        assert_eq!(
            click(&mut state, send.x + 1, send.y),
            [Job::Comment {
                key: "VK25-3".into(),
                text: "oi".into()
            }]
        );

        state.popup = None;
        state.key(press(KeyCode::Char('c')));
        draw(&mut state, 60, 16);
        let cancel = rect_of(&state, &Hit::Button(Action::Cancel));
        click(&mut state, cancel.x + 1, cancel.y);
        assert!(state.popup.is_none());
    }

    #[test]
    fn detail_renders_fields_description_comments_and_clickable_links() {
        let mut state = loaded_state();
        state.key(press(KeyCode::Enter));
        let detail = IssueDetail {
            issue: crate::cli::jira::state::tests::issue("VK25-3", "Code Review"),
            description: Some(serde_json::json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "PR aqui", "marks": [{"type": "link", "attrs": {"href": "https://github.com/pr/1"}}]}]}
            ]})),
            comments: vec![super::super::model::Comment {
                author: "Ana".into(),
                created: "2020-01-01T00:00:00.000+0000".into(),
                body: serde_json::json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "ok"}]}]}),
            }],
            sprint: Some("Sprint 9".into()),
            story_points: Some(2.0),
            development: Some("1 PR (open)".into()),
            pull_requests: vec![super::super::model::PullRequest {
                status: "OPEN".into(),
                title: "VK25-3: tela nova".into(),
                url: "https://github.com/vakinha/vakinha-web/pull/5783".into(),
                repository: "vakinha/vakinha-web".into(),
                branch: Some("task/VK25-3/tela".into()),
            }],
            worktrees: vec![
                super::super::model::Worktree {
                    repo: "vakinha-api".into(),
                    name: "VK25-3-api".into(),
                    path: "/r/vakinha-api-worktrees/VK25-3-api".into(),
                    branch: Some("task/VK25-3/api".into()),
                },
                super::super::model::Worktree {
                    repo: "vakinha-web".into(),
                    name: "VK25-3".into(),
                    path: "/r/vakinha-web-worktrees/VK25-3".into(),
                    branch: Some("task/VK25-3/tela".into()),
                },
            ],
        };
        state.apply(Outcome::Detail(Box::new(detail)));
        let terminal = draw(&mut state, 70, 40);
        let text = screen(&terminal);

        for expected in [
            "VK25-3 · Task · Code Review",
            "▾ pull requests (1)",
            "open  vakinha/vakinha-web#5783",
            "▾ worktrees (2)",
            "vakinha-api  VK25-3-api",
            "vakinha-web  VK25-3 → #5783",
            "task/VK25-3/tela",
            "sprint: Sprint 9",
            "pontos: 2",
            "desenvolvimento: 1 PR (open)",
            "Descrição",
            "Comentários (1)",
            "Ana · 2020-01-01",
            "esc voltar",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert!(!text.contains("vakinha-api  VK25-3-api → #"));
        let link = rect_of(&state, &Hit::Link("https://github.com/pr/1".into()));
        assert_eq!(
            click(&mut state, link.x, link.y),
            [Job::OpenUrl("https://github.com/pr/1".into())]
        );
        let worktree = rect_of(&state, &Hit::Worktree(1));
        assert_eq!(
            click(&mut state, worktree.x, worktree.y),
            [Job::OpenWorktree {
                path: "/r/vakinha-web-worktrees/VK25-3".into(),
                name: "VK25-3".into(),
            }]
        );

        state.key(press(KeyCode::Char('t')));
        let text = screen(&draw(&mut state, 70, 40));
        assert!(text.contains("▸ worktrees (2)"));
        assert!(!text.contains("VK25-3-api"));
        let header = rect_of(&state, &Hit::Section(DetailSection::PullRequests));
        click(&mut state, header.x, header.y);
        let text = screen(&draw(&mut state, 70, 40));
        assert!(text.contains("▸ pull requests (1)"));
        assert!(!text.contains("vakinha/vakinha-web#5783"));
        state.key(press(KeyCode::Char('p')));
        state.key(press(KeyCode::Char('t')));
        let text = screen(&draw(&mut state, 70, 40));
        assert!(text.contains("▾ pull requests (1)") && text.contains("▾ worktrees (2)"));
    }

    #[test]
    fn empty_sections_render_their_count_and_nenhum() {
        let mut lines_state = HashSet::new();
        let detail = IssueDetail {
            issue: crate::cli::jira::state::tests::issue("VK25-3", "Code Review"),
            description: None,
            comments: Vec::new(),
            sprint: None,
            story_points: None,
            development: None,
            pull_requests: Vec::new(),
            worktrees: Vec::new(),
        };
        let text = |collapsed: &HashSet<DetailSection>| {
            detail_lines("VK25-3", Some(&detail), None, "", collapsed, Instant::now())
                .iter()
                .map(RichLine::text)
                .collect::<Vec<_>>()
        };
        let open = text(&lines_state);
        assert!(open.contains(&"▾ pull requests (0)  p".to_owned()));
        assert!(open.contains(&"▾ worktrees (0)  t".to_owned()));
        assert_eq!(
            open.iter().filter(|line| line.trim() == "nenhum").count(),
            2
        );
        lines_state.insert(DetailSection::Worktrees);
        let closed = text(&lines_state);
        assert!(closed.contains(&"▸ worktrees (0)  t".to_owned()));
        assert_eq!(
            closed.iter().filter(|line| line.trim() == "nenhum").count(),
            1
        );
    }

    #[test]
    fn internal_links_map_to_section_and_worktree_hits() {
        assert_eq!(
            link_hit(&section_link(DetailSection::Worktrees)),
            Hit::Section(DetailSection::Worktrees)
        );
        assert_eq!(link_hit("herdr-jira:worktree:3"), Hit::Worktree(3));
        assert_eq!(link_hit("https://x/y"), Hit::Link("https://x/y".into()));
    }

    #[test]
    fn buttons_wrap_on_narrow_panels() {
        let placed = layout_buttons(&LIST_ACTIONS, 30);

        assert!(placed.iter().any(|(_, row, _)| *row > 0));
        for (action, _, x) in placed {
            assert!(x + action.label().width() as u16 + 2 <= 30);
        }
    }

    #[test]
    fn editor_wraps_and_tracks_cursor() {
        let (rows, cursor) = wrap_editor("abcdef\ngh", 9, 4);

        assert_eq!(rows, ["abcd", "ef", "gh"]);
        assert_eq!(cursor, (2, 2));
        assert_eq!(wrap_editor("ab", 1, 4).1, (0, 1));
    }

    #[test]
    fn jira_timestamps_parse_with_offsets() {
        assert_eq!(
            parse_jira_time("2026-09-23T10:00:00.000-0300"),
            Some(1_790_168_400)
        );
        assert_eq!(
            parse_jira_time("2026-09-23T13:00:00.000+0000"),
            Some(1_790_168_400)
        );
        assert_eq!(parse_jira_time("garbage"), None);
    }
}
