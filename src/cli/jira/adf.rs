use serde_json::{json, Value};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub underline: bool,
    pub heading: bool,
    pub quote: bool,
    pub mention: bool,
    pub dim: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RichSpan {
    pub text: String,
    pub style: SpanStyle,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RichLine {
    pub indent: usize,
    pub prefix: String,
    pub spans: Vec<RichSpan>,
}

impl RichLine {
    pub(crate) fn plain(text: impl Into<String>, style: SpanStyle) -> Self {
        Self {
            indent: 0,
            prefix: String::new(),
            spans: vec![RichSpan {
                text: text.into(),
                style,
                link: None,
            }],
        }
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        let mut text = format!("{}{}", " ".repeat(self.indent), self.prefix);
        for span in &self.spans {
            text.push_str(&span.text);
        }
        text
    }
}

pub(crate) fn render(doc: &Value) -> Vec<RichLine> {
    let mut renderer = Renderer::default();
    renderer.block(doc, 0);
    renderer.flush();
    while renderer
        .lines
        .last()
        .is_some_and(|line| line.spans.is_empty())
    {
        renderer.lines.pop();
    }
    renderer.lines
}

#[derive(Default)]
struct Renderer {
    lines: Vec<RichLine>,
    current: Option<RichLine>,
}

impl Renderer {
    fn flush(&mut self) {
        if let Some(line) = self.current.take() {
            self.lines.push(line);
        }
    }

    fn start_line(&mut self, indent: usize, prefix: String) {
        self.flush();
        self.current = Some(RichLine {
            indent,
            prefix,
            spans: Vec::new(),
        });
    }

    fn blank(&mut self) {
        self.flush();
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.lines.push(RichLine::default());
        }
    }

    fn push(&mut self, text: &str, style: SpanStyle, link: Option<String>) {
        if self.current.is_none() {
            self.start_line(0, String::new());
        }
        let line = self.current.as_mut().unwrap();
        line.spans.push(RichSpan {
            text: text.to_owned(),
            style,
            link,
        });
    }

    fn children(&mut self, node: &Value, indent: usize) {
        for child in content(node) {
            self.block(child, indent);
        }
    }

    fn block(&mut self, node: &Value, indent: usize) {
        match node_type(node) {
            "doc" => self.children(node, indent),
            "paragraph" => {
                self.start_line(indent, String::new());
                self.inline_children(node, SpanStyle::default());
                self.blank();
            }
            "heading" => {
                self.start_line(indent, String::new());
                self.inline_children(
                    node,
                    SpanStyle {
                        bold: true,
                        heading: true,
                        ..SpanStyle::default()
                    },
                );
                self.blank();
            }
            "bulletList" | "orderedList" => {
                let ordered = node_type(node) == "orderedList";
                let start = node
                    .get("attrs")
                    .and_then(|attrs| attrs.get("order"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                for (index, item) in content(node).iter().enumerate() {
                    let marker = if ordered {
                        format!("{}. ", start + index as u64)
                    } else {
                        "• ".to_owned()
                    };
                    self.list_item(item, indent, marker);
                }
                self.blank();
            }
            "codeBlock" => {
                let text = collect_text(node);
                let style = SpanStyle {
                    code: true,
                    ..SpanStyle::default()
                };
                for line in text.lines() {
                    self.start_line(indent + 2, String::new());
                    self.push(line, style, None);
                }
                self.blank();
            }
            "blockquote" => {
                let before = self.lines.len();
                self.children(node, indent);
                self.flush();
                for line in &mut self.lines[before..] {
                    if !line.spans.is_empty() {
                        line.prefix = format!("│ {}", line.prefix);
                        for span in &mut line.spans {
                            span.style.quote = true;
                        }
                    }
                }
            }
            "panel" => {
                let panel_type = node
                    .get("attrs")
                    .and_then(|attrs| attrs.get("panelType"))
                    .and_then(Value::as_str)
                    .unwrap_or("info");
                let before = self.lines.len();
                self.children(node, indent);
                self.flush();
                if let Some(first) = self.lines[before..]
                    .iter_mut()
                    .find(|line| !line.spans.is_empty())
                {
                    first.prefix = format!("[{panel_type}] {}", first.prefix);
                }
            }
            "rule" => {
                self.start_line(indent, String::new());
                self.push(
                    "────────",
                    SpanStyle {
                        dim: true,
                        ..SpanStyle::default()
                    },
                    None,
                );
                self.blank();
            }
            "table" => {
                for row in content(node) {
                    self.start_line(indent, String::new());
                    for (index, cell) in content(row).iter().enumerate() {
                        let header = node_type(cell) == "tableHeader";
                        if index > 0 {
                            self.push(
                                " │ ",
                                SpanStyle {
                                    dim: true,
                                    ..SpanStyle::default()
                                },
                                None,
                            );
                        }
                        let text = collect_text(cell);
                        self.push(
                            text.trim(),
                            SpanStyle {
                                bold: header,
                                ..SpanStyle::default()
                            },
                            None,
                        );
                    }
                }
                self.blank();
            }
            "mediaSingle" | "mediaGroup" | "media" => {
                self.start_line(indent, String::new());
                self.push(
                    "[mídia]",
                    SpanStyle {
                        dim: true,
                        ..SpanStyle::default()
                    },
                    None,
                );
                self.blank();
            }
            "text" | "hardBreak" | "mention" | "emoji" | "inlineCard" | "date" | "status" => {
                self.inline(node, SpanStyle::default());
            }
            _ => {
                if content(node).is_empty() {
                    let text = collect_text(node);
                    if !text.trim().is_empty() {
                        self.start_line(indent, String::new());
                        self.push(text.trim(), SpanStyle::default(), None);
                        self.blank();
                    }
                } else {
                    self.children(node, indent);
                }
            }
        }
    }

    fn list_item(&mut self, item: &Value, indent: usize, marker: String) {
        let mut first = true;
        for child in content(item) {
            match node_type(child) {
                "paragraph" => {
                    let prefix = if first {
                        marker.clone()
                    } else {
                        " ".repeat(marker.chars().count())
                    };
                    self.start_line(indent, prefix);
                    self.inline_children(child, SpanStyle::default());
                    self.flush();
                }
                "bulletList" | "orderedList" => {
                    if first {
                        self.start_line(indent, marker.clone());
                        self.flush();
                    }
                    self.block(child, indent + 2);
                    if self.lines.last().is_some_and(|line| line.spans.is_empty()) {
                        self.lines.pop();
                    }
                }
                _ => {
                    self.block(child, indent + marker.chars().count());
                }
            }
            first = false;
        }
        if first {
            self.start_line(indent, marker);
            self.flush();
        }
    }

    fn inline_children(&mut self, node: &Value, base: SpanStyle) {
        for child in content(node) {
            self.inline(child, base);
        }
    }

    fn inline(&mut self, node: &Value, base: SpanStyle) {
        let attrs = node.get("attrs");
        let attr = |key: &str| {
            attrs
                .and_then(|attrs| attrs.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        match node_type(node) {
            "text" => {
                let text = node.get("text").and_then(Value::as_str).unwrap_or_default();
                let mut style = base;
                let mut link = None;
                for mark in node
                    .get("marks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match node_type(mark) {
                        "strong" => style.bold = true,
                        "em" => style.italic = true,
                        "code" => style.code = true,
                        "strike" => style.strike = true,
                        "underline" => style.underline = true,
                        "link" => {
                            style.underline = true;
                            link = mark
                                .get("attrs")
                                .and_then(|attrs| attrs.get("href"))
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                        }
                        _ => {}
                    }
                }
                let mut pieces = text.split('\n');
                if let Some(first) = pieces.next() {
                    self.push(first, style, link.clone());
                }
                for piece in pieces {
                    let (indent, prefix) = self.continuation();
                    self.start_line(indent, prefix);
                    self.push(piece, style, link.clone());
                }
            }
            "hardBreak" => {
                let (indent, prefix) = self.continuation();
                self.start_line(indent, prefix);
            }
            "mention" => {
                let text = attr("text").unwrap_or_else(|| "@alguém".to_owned());
                let text = if text.starts_with('@') {
                    text
                } else {
                    format!("@{text}")
                };
                self.push(
                    &text,
                    SpanStyle {
                        mention: true,
                        ..base
                    },
                    None,
                );
            }
            "emoji" => {
                let text = attr("text")
                    .or_else(|| attr("shortName"))
                    .unwrap_or_default();
                self.push(&text, base, None);
            }
            "inlineCard" => {
                let url = attr("url").unwrap_or_default();
                self.push(
                    &url,
                    SpanStyle {
                        underline: true,
                        ..base
                    },
                    Some(url.clone()),
                );
            }
            "date" => {
                let text = attr("timestamp")
                    .and_then(|timestamp| timestamp.parse::<i64>().ok())
                    .and_then(|millis| {
                        time::OffsetDateTime::from_unix_timestamp(millis / 1000).ok()
                    })
                    .map(|date| {
                        format!(
                            "{:04}-{:02}-{:02}",
                            date.year(),
                            u8::from(date.month()),
                            date.day()
                        )
                    })
                    .unwrap_or_default();
                self.push(&text, base, None);
            }
            "status" => {
                let text = attr("text").unwrap_or_default();
                self.push(&format!("[{text}]"), SpanStyle { bold: true, ..base }, None);
            }
            _ => {
                let text = collect_text(node);
                if !text.is_empty() {
                    self.push(&text, base, None);
                }
            }
        }
    }

    fn continuation(&self) -> (usize, String) {
        self.current
            .as_ref()
            .map(|line| (line.indent, " ".repeat(line.prefix.chars().count())))
            .unwrap_or_default()
    }
}

fn node_type(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn content(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn collect_text(node: &Value) -> String {
    if let Some(text) = node.get("text").and_then(Value::as_str) {
        return text.to_owned();
    }
    match node_type(node) {
        "hardBreak" => return "\n".to_owned(),
        "mention" => {
            return node
                .get("attrs")
                .and_then(|attrs| attrs.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        }
        _ => {}
    }
    let parts: Vec<String> = content(node).iter().map(collect_text).collect();
    let separator = if content(node)
        .iter()
        .any(|child| matches!(node_type(child), "paragraph" | "codeBlock" | "listItem"))
    {
        "\n"
    } else {
        ""
    };
    parts.join(separator)
}

pub(crate) fn text_to_adf(text: &str) -> Value {
    let paragraphs: Vec<Value> = text
        .trim_end()
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                json!({ "type": "paragraph", "content": [] })
            } else {
                json!({ "type": "paragraph", "content": [{ "type": "text", "text": line }] })
            }
        })
        .collect();
    json!({ "type": "doc", "version": 1, "content": paragraphs })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WrappedSpan {
    pub text: String,
    pub style: SpanStyle,
    pub link: Option<String>,
}

pub(crate) fn wrap(lines: &[RichLine], width: usize) -> Vec<Vec<WrappedSpan>> {
    let width = width.max(8);
    let mut out = Vec::new();
    for line in lines {
        let lead = format!("{}{}", " ".repeat(line.indent), line.prefix);
        let hanging = " ".repeat(lead.width().min(width / 2));
        let mut row: Vec<WrappedSpan> = Vec::new();
        let mut used = 0;
        if !lead.is_empty() {
            used = lead.width();
            row.push(WrappedSpan {
                text: lead.clone(),
                style: SpanStyle::default(),
                link: None,
            });
        }
        if line.spans.is_empty() {
            out.push(row);
            continue;
        }
        for span in &line.spans {
            for word in split_keep_spaces(&span.text) {
                let word_width = word.width();
                if used + word_width > width && used > hanging.width() {
                    out.push(std::mem::take(&mut row));
                    used = 0;
                    if !hanging.is_empty() {
                        row.push(WrappedSpan {
                            text: hanging.clone(),
                            style: SpanStyle::default(),
                            link: None,
                        });
                        used = hanging.width();
                    }
                    if word.trim().is_empty() {
                        continue;
                    }
                }
                let mut piece = word.to_owned();
                while used + piece.width() > width {
                    let fit = width.saturating_sub(used).max(1);
                    let split = char_boundary_for_width(&piece, fit);
                    let rest = piece.split_off(split);
                    push_piece(&mut row, &piece, span);
                    out.push(std::mem::take(&mut row));
                    used = 0;
                    piece = rest;
                }
                used += piece.width();
                push_piece(&mut row, &piece, span);
            }
        }
        out.push(row);
    }
    out
}

fn push_piece(row: &mut Vec<WrappedSpan>, text: &str, span: &RichSpan) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = row.last_mut() {
        if last.style == span.style && last.link == span.link {
            last.text.push_str(text);
            return;
        }
    }
    row.push(WrappedSpan {
        text: text.to_owned(),
        style: span.style,
        link: span.link.clone(),
    });
}

fn split_keep_spaces(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_space = None;
    for (index, ch) in text.char_indices() {
        let space = ch == ' ';
        match in_space {
            Some(previous) if previous != space => {
                parts.push(&text[start..index]);
                start = index;
            }
            _ => {}
        }
        in_space = Some(space);
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

fn char_boundary_for_width(text: &str, width: usize) -> usize {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            return index.max(ch.len_utf8().min(text.len()));
        }
        used += ch_width;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: Value) -> Vec<String> {
        render(&doc).iter().map(RichLine::text).collect()
    }

    fn text_node(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    #[test]
    fn paragraphs_and_headings_are_separated_by_blank_lines() {
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"level": 2}, "content": [text_node("Título")]},
            {"type": "paragraph", "content": [text_node("um "), {"type": "text", "text": "negrito", "marks": [{"type": "strong"}]}]},
        ]});

        let lines = render(&doc);

        assert_eq!(texts(doc), ["Título", "", "um negrito"]);
        assert!(lines[0].spans[0].style.heading);
        assert!(lines[2].spans[1].style.bold);
    }

    #[test]
    fn lists_are_numbered_and_nested() {
        let item = |text: &str| json!({"type": "listItem", "content": [{"type": "paragraph", "content": [text_node(text)]}]});
        let doc = json!({"type": "doc", "content": [
            {"type": "orderedList", "attrs": {"order": 3}, "content": [item("a"), {"type": "listItem", "content": [
                {"type": "paragraph", "content": [text_node("b")]},
                {"type": "bulletList", "content": [item("b1")]},
            ]}]},
        ]});

        assert_eq!(texts(doc), ["3. a", "4. b", "  • b1"]);
    }

    #[test]
    fn code_blocks_marks_links_and_mentions() {
        let doc = json!({"type": "doc", "content": [
            {"type": "codeBlock", "content": [text_node("let x = 1;\nlet y = 2;")]},
            {"type": "paragraph", "content": [
                {"type": "text", "text": "PR", "marks": [{"type": "link", "attrs": {"href": "https://github.com/x/1"}}]},
                text_node(" por "),
                {"type": "mention", "attrs": {"text": "@Ana"}},
                text_node(" "),
                {"type": "text", "text": "cmd", "marks": [{"type": "code"}]},
                {"type": "hardBreak"},
                text_node("depois"),
            ]},
        ]});

        let lines = render(&doc);

        assert_eq!(
            lines.iter().map(RichLine::text).collect::<Vec<_>>(),
            [
                "  let x = 1;",
                "  let y = 2;",
                "",
                "PR por @Ana cmd",
                "depois"
            ]
        );
        assert!(lines[0].spans[0].style.code);
        assert_eq!(
            lines[3].spans[0].link.as_deref(),
            Some("https://github.com/x/1")
        );
        assert!(lines[3].spans[2].style.mention);
        assert!(lines[3].spans[4].style.code);
    }

    #[test]
    fn tables_quotes_panels_rules_and_media() {
        let cell = |kind: &str, text: &str| json!({"type": kind, "content": [{"type": "paragraph", "content": [text_node(text)]}]});
        let doc = json!({"type": "doc", "content": [
            {"type": "table", "content": [
                {"type": "tableRow", "content": [cell("tableHeader", "col"), cell("tableHeader", "val")]},
                {"type": "tableRow", "content": [cell("tableCell", "a"), cell("tableCell", "1")]},
            ]},
            {"type": "blockquote", "content": [{"type": "paragraph", "content": [text_node("citação")]}]},
            {"type": "panel", "attrs": {"panelType": "warning"}, "content": [{"type": "paragraph", "content": [text_node("cuidado")]}]},
            {"type": "rule"},
            {"type": "mediaSingle", "content": [{"type": "media"}]},
        ]});

        let lines = render(&doc);
        let text: Vec<_> = lines.iter().map(RichLine::text).collect();

        assert_eq!(text[0], "col │ val");
        assert_eq!(text[1], "a │ 1");
        assert!(text.contains(&"│ citação".to_owned()));
        assert!(text.contains(&"[warning] cuidado".to_owned()));
        assert!(text.contains(&"────────".to_owned()));
        assert!(text.contains(&"[mídia]".to_owned()));
        assert!(lines[0].spans[0].style.bold);
    }

    #[test]
    fn unknown_nodes_fall_back_to_their_text() {
        let doc = json!({"type": "doc", "content": [
            {"type": "expand", "content": [{"type": "paragraph", "content": [text_node("dentro")]}]},
            {"type": "weirdLeaf", "text": "folha"},
            {"type": "paragraph", "content": [{"type": "status", "attrs": {"text": "FEITO"}}, {"type": "date", "attrs": {"timestamp": "1758585600000"}}]},
        ]});

        assert_eq!(texts(doc), ["dentro", "", "folha", "", "[FEITO]2025-09-23"]);
    }

    #[test]
    fn text_to_adf_builds_one_paragraph_per_line() {
        let doc = text_to_adf("oi\n\ntchau\n");

        assert_eq!(
            doc,
            json!({"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "oi"}]},
                {"type": "paragraph", "content": []},
                {"type": "paragraph", "content": [{"type": "text", "text": "tchau"}]},
            ]})
        );
    }

    #[test]
    fn wrap_breaks_on_spaces_keeps_hanging_indent_and_links() {
        let line = RichLine {
            indent: 0,
            prefix: "• ".into(),
            spans: vec![
                RichSpan {
                    text: "abc def ".into(),
                    style: SpanStyle::default(),
                    link: None,
                },
                RichSpan {
                    text: "ghijkl".into(),
                    style: SpanStyle::default(),
                    link: Some("u".into()),
                },
            ],
        };

        let rows = wrap(&[line], 10);
        let text: Vec<String> = rows
            .iter()
            .map(|row| row.iter().map(|span| span.text.as_str()).collect())
            .collect();

        assert_eq!(text, ["• abc def ", "  ghijkl"]);
        assert_eq!(rows[1].last().unwrap().link.as_deref(), Some("u"));
    }

    #[test]
    fn wrap_splits_words_longer_than_the_width() {
        let rows = wrap(
            &[RichLine::plain("abcdefghijklmnop", SpanStyle::default())],
            8,
        );
        let text: Vec<String> = rows
            .iter()
            .map(|row| row.iter().map(|span| span.text.as_str()).collect())
            .collect();

        assert_eq!(text, ["abcdefgh", "ijklmnop"]);
    }
}
