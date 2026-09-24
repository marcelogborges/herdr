use std::process::Stdio;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GcReport {
    pub summary: Option<String>,
    pub removable: Vec<String>,
    pub orphans: Vec<String>,
    pub lines: Vec<String>,
}

impl GcReport {
    pub(crate) fn has_work(&self) -> bool {
        !self.removable.is_empty() || !self.orphans.is_empty()
    }
}

pub(crate) fn command(base: &str, apply: bool) -> String {
    let base = base.trim();
    if apply {
        format!("{base} --apply")
    } else {
        base.to_owned()
    }
}

pub(crate) fn run(base: &str, apply: bool) -> Result<GcReport, String> {
    let command = command(base, apply);
    let output = crate::noninteractive_process::command("/bin/sh")
        .args(["-c", &command])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("não foi possível rodar `{command}`: {err}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    let report = parse(&text);
    if output.status.success() {
        Ok(report)
    } else {
        let tail = report
            .lines
            .iter()
            .rev()
            .take(3)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        let code = output
            .status
            .code()
            .map_or_else(|| "sinal".to_owned(), |code| code.to_string());
        Err(if tail.is_empty() {
            format!("`{command}` saiu com {code}")
        } else {
            format!("`{command}` saiu com {code}: {tail}")
        })
    }
}

pub(crate) fn parse(raw: &str) -> GcReport {
    let text = strip_ansi(raw);
    let mut report = GcReport::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        report.lines.push(trimmed.to_owned());
        if let Some(rest) = trimmed.strip_prefix("LIXO ") {
            report.removable.push(squash(rest));
        } else if let Some(rest) = trimmed.strip_prefix("ORFA ") {
            report.orphans.push(squash(rest));
        } else if trimmed.contains(" worktrees | ") || trimmed.contains(" worktrees removidas") {
            report.summary = Some(trimmed.to_owned());
        }
    }
    report
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join("  ")
}

pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some('(' | ')' | '*' | '+') => {
                chars.next();
                chars.next();
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRY_RUN: &str = "\n--- LIXO ---\n  \u{1b}[32mLIXO\u{1b}[0m  vakinha-web-worktrees/VK25-2811                      pr=merged                                1650MB\n\n--- ORFA ---\n  \u{1b}[33mORFA\u{1b}[0m  vakinha-api-worktrees/VK25-2001                      diretorio-ausente                        0MB\n\n--- KEEP ---\n  \u{1b}[31mKEEP\u{1b}[0m  vakinha-admin-api-worktrees/VK25-2907                pr=open                                  47MB\n  \u{1b}[31mKEEP\u{1b}[0m  vakinha-api-worktrees/VK25-2640                      pr=closed                                87MB\n\n4 worktrees | 1 removiveis (~1650MB) | 1 orfas | 2 mantidas\nrelatorio: /home/me/projects/extras/dev/PENDENTES.md\n\u{1b}[2mdry-run. para executar: vk-wt gc --apply\u{1b}[0m\n";

    const NOTHING: &str = "\n--- KEEP ---\n  \u{1b}[31mKEEP\u{1b}[0m  vakinha-web-worktrees/VK25-2908                      pr=open                                  39MB\n\n30 worktrees | 0 removiveis (~0MB) | 0 orfas | 30 mantidas\nrelatorio: /home/me/projects/extras/dev/PENDENTES.md\n\u{1b}[2mdry-run. para executar: vk-wt gc --apply\u{1b}[0m\n";

    #[test]
    fn parses_summary_removable_and_orphan_rows() {
        let report = parse(DRY_RUN);
        assert_eq!(
            report.summary.as_deref(),
            Some("4 worktrees | 1 removiveis (~1650MB) | 1 orfas | 2 mantidas")
        );
        assert_eq!(
            report.removable,
            ["vakinha-web-worktrees/VK25-2811  pr=merged  1650MB"]
        );
        assert_eq!(
            report.orphans,
            ["vakinha-api-worktrees/VK25-2001  diretorio-ausente  0MB"]
        );
        assert!(report.has_work());
        assert!(report.lines.iter().all(|line| !line.contains('\u{1b}')));
    }

    #[test]
    fn nothing_to_remove_has_no_work() {
        let report = parse(NOTHING);
        assert!(!report.has_work());
        assert_eq!(
            report.summary.as_deref(),
            Some("30 worktrees | 0 removiveis (~0MB) | 0 orfas | 30 mantidas")
        );
    }

    #[test]
    fn apply_summary_is_recognised() {
        let report = parse(
            "  \u{1b}[31mfalhou\u{1b}[0m /x: busy\n1 worktrees removidas, 0 orfas podadas.\n",
        );
        assert_eq!(
            report.summary.as_deref(),
            Some("1 worktrees removidas, 0 orfas podadas.")
        );
        assert_eq!(report.lines[0], "falhou /x: busy");
    }

    #[test]
    fn strips_csi_osc_and_lone_escapes() {
        assert_eq!(
            strip_ansi(
                "\u{1b}[1;31mred\u{1b}[0m \u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{1b}\\ \u{1b}(Bok"
            ),
            "red link ok"
        );
    }

    #[test]
    fn apply_appends_the_flag_to_the_configured_command() {
        assert_eq!(command("vk-wt gc", false), "vk-wt gc");
        assert_eq!(command(" vk-wt gc ", true), "vk-wt gc --apply");
    }

    #[test]
    fn runs_the_command_and_reports_failures() {
        let dir = std::env::temp_dir().join(format!("herdr-gc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-gc");
        std::fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1\" = \"--apply\" ]; then echo '1 worktrees removidas, 0 orfas podadas.'; exit 0; fi\nif [ \"$1\" = \"--boom\" ]; then echo 'deu ruim' >&2; exit 3; fi\nprintf '  \\033[32mLIXO\\033[0m  a-worktrees/K-1  pr=merged  1MB\\n1 worktrees | 1 removiveis (~1MB) | 0 orfas | 0 mantidas\\n'\n",
        )
        .unwrap();
        let base = format!("sh '{}'", script.display());
        let dry = run(&base, false).unwrap();
        assert_eq!(dry.removable, ["a-worktrees/K-1  pr=merged  1MB"]);
        let applied = run(&base, true).unwrap();
        assert_eq!(
            applied.summary.as_deref(),
            Some("1 worktrees removidas, 0 orfas podadas.")
        );
        let error = run(&format!("{base} --boom"), false).unwrap_err();
        assert!(error.contains("saiu com 3"), "{error}");
        assert!(error.contains("deu ruim"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
