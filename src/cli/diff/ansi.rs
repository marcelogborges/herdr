use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const TAB_WIDTH: usize = 8;

pub(crate) fn parse(bytes: &[u8]) -> Vec<Line<'static>> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = Vec::new();
    let mut style = Style::default();
    for raw in text.split('\n') {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current = String::new();
        let mut column = 0usize;
        let mut chars = raw.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut final_byte = None;
                        for next in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&next) {
                                final_byte = Some(next);
                                break;
                            }
                            params.push(next);
                        }
                        if final_byte == Some('m') {
                            if !current.is_empty() {
                                spans.push(Span::styled(std::mem::take(&mut current), style));
                            }
                            style = apply_sgr(style, &params);
                        }
                    }
                    Some(']') => {
                        while let Some(next) = chars.next() {
                            if next == '\x07' {
                                break;
                            }
                            if next == '\x1b' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\t' => {
                    let pad = TAB_WIDTH - column % TAB_WIDTH;
                    current.push_str(&" ".repeat(pad));
                    column += pad;
                }
                '\r' => {}
                ch if ch.is_control() => {}
                ch => {
                    current.push(ch);
                    column += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                }
            }
        }
        if !current.is_empty() {
            spans.push(Span::styled(current, style));
        }
        lines.push(Line::from(spans));
    }
    if lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    lines
}

fn apply_sgr(mut style: Style, params: &str) -> Style {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params
            .split([';', ':'])
            .map(|code| code.parse().unwrap_or(0))
            .collect()
    };
    let mut index = 0;
    while index < codes.len() {
        let code = codes[index];
        match code {
            0 => style = Style::default(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            9 => style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            29 => style = style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => style = style.fg(Color::Indexed((code - 30) as u8)),
            39 => style.fg = None,
            40..=47 => style = style.bg(Color::Indexed((code - 40) as u8)),
            49 => style.bg = None,
            90..=97 => style = style.fg(Color::Indexed((code - 90 + 8) as u8)),
            100..=107 => style = style.bg(Color::Indexed((code - 100 + 8) as u8)),
            38 | 48 => {
                let (color, used) = extended_color(&codes[index + 1..]);
                if let Some(color) = color {
                    style = if code == 38 {
                        style.fg(color)
                    } else {
                        style.bg(color)
                    };
                }
                index += used;
            }
            _ => {}
        }
        index += 1;
    }
    style
}

fn extended_color(rest: &[u16]) -> (Option<Color>, usize) {
    match rest {
        [5, value, ..] => (Some(Color::Indexed(*value as u8)), 2),
        [2, r, g, b, ..] => (Some(Color::Rgb(*r as u8, *g as u8, *b as u8)), 4),
        [5] | [2, ..] => (None, rest.len()),
        _ => (None, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_sequences_become_styled_spans() {
        let lines = parse(
            b"\x1b[1;38;2;10;20;30mbold\x1b[0m plain\n\x1b[48;5;22m\x1b[31mred\x1b[39mbg\x1b[m\n",
        );

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].content, "bold");
        assert_eq!(
            lines[0].spans[0].style,
            Style::default()
                .fg(Color::Rgb(10, 20, 30))
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(lines[0].spans[1].style, Style::default());
        assert_eq!(
            lines[1].spans[0].style,
            Style::default()
                .fg(Color::Indexed(1))
                .bg(Color::Indexed(22))
        );
        assert_eq!(
            lines[1].spans[1].style,
            Style::default().bg(Color::Indexed(22))
        );
    }

    #[test]
    fn style_carries_across_lines_and_other_escapes_are_dropped() {
        let lines =
            parse(b"\x1b[32mgreen\x1b[K\nstill\x1b]8;;http://x\x1b\\link\x1b]8;;\x07\n\ta\r");

        assert_eq!(lines[0].spans[0].content, "green");
        assert_eq!(lines[1].spans[0].content, "stilllink");
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Indexed(2)));
        assert_eq!(lines[2].spans[0].content, "        a");
    }
}
