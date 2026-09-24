pub(crate) mod adf;
pub(crate) mod api;
pub(crate) mod herdr;
pub(crate) mod model;
pub(crate) mod state;
pub(crate) mod ui;
pub(crate) mod worker;

use std::io::{self, Stdout};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Terminal;

use crate::config::JiraConfig;

use self::api::{DetailFields, JiraApi};
use self::state::JiraState;
use self::worker::{Job, WorkerContext};

const POLL: Duration = Duration::from_millis(150);
const OWNER_PANE_ENVS: [&str; 2] = ["HERDR_PANEL_OWNER_PANE_ID", "HERDR_ACTIVE_PANE_ID"];

pub(super) fn run_jira_command(args: &[String]) -> io::Result<i32> {
    if let Some(arg) = args.first() {
        if matches!(arg.as_str(), "help" | "--help" | "-h") {
            print_help();
            return Ok(0);
        }
        eprintln!("unexpected argument: {arg}");
        print_help();
        return Ok(2);
    }
    let config = crate::config::Config::load().config.jira;
    let setup = resolve_setup(&config, |name| std::env::var(name).ok());
    let mut terminal = enter_terminal()?;
    let result = match setup {
        Ok(setup) => run_tui(&mut terminal, setup),
        Err(problems) => run_error_screen(&mut terminal, &problems),
    };
    leave_terminal(&mut terminal)?;
    result.map(|()| 0)
}

fn print_help() {
    eprintln!("herdr jira: interactive Jira board for the right panel");
    eprintln!("  usage: herdr jira");
    eprintln!(
        "  config: [jira] site, email, project, token_env (plus jql, status_order, worktree_roots, refresh_seconds)"
    );
}

pub(crate) struct Setup {
    pub api: JiraApi,
    pub jql: String,
    pub project: String,
    pub browse_base: String,
    pub status_order: Vec<String>,
    pub refresh_seconds: u64,
    pub owner_pane: Option<String>,
    pub worktree_roots: Vec<String>,
    pub browser_command: String,
}

pub(crate) fn resolve_setup(
    config: &JiraConfig,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Setup, Vec<String>> {
    let mut problems = Vec::new();
    if config.site.trim().is_empty() {
        problems.push("[jira] site = \"https://<sua-empresa>.atlassian.net\"".to_owned());
    }
    if config.email.trim().is_empty() {
        problems.push("[jira] email = \"<seu email do Atlassian>\"".to_owned());
    }
    if config.project.trim().is_empty() && config.jql.trim().is_empty() {
        problems.push("[jira] project = \"<CHAVE>\" (ou [jira] jql = \"...\")".to_owned());
    }
    let token_env = if config.token_env.trim().is_empty() {
        "JIRA_API_TOKEN"
    } else {
        config.token_env.trim()
    };
    let token = env(token_env).filter(|token| !token.trim().is_empty());
    if token.is_none() {
        problems.push(format!(
            "variável de ambiente {token_env} com o API token (id.atlassian.com → Security → API tokens), visível para o servidor do herdr"
        ));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let site = config.site.trim().trim_end_matches('/').to_owned();
    let jql = if config.jql.trim().is_empty() {
        format!(
            "project = {} AND assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC",
            config.project.trim()
        )
    } else {
        config.jql.trim().to_owned()
    };
    Ok(Setup {
        api: JiraApi::new(
            &site,
            config.email.trim(),
            token.unwrap_or_default(),
            DetailFields {
                sprint: config.sprint_field.clone(),
                story_points: config.story_points_field.clone(),
                development: config.development_field.clone(),
            },
        ),
        jql,
        project: if config.project.trim().is_empty() {
            "jql".to_owned()
        } else {
            config.project.trim().to_owned()
        },
        browse_base: format!("{site}/browse/"),
        status_order: config.status_order.clone(),
        refresh_seconds: config.refresh_seconds,
        owner_pane: OWNER_PANE_ENVS
            .iter()
            .find_map(|name| env(name).filter(|value| !value.is_empty())),
        worktree_roots: config.worktree_roots.clone(),
        browser_command: config.browser_command.clone(),
    })
}

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn enter_terminal() -> io::Result<Tui> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        previous(info);
    }));
    Terminal::new(CrosstermBackend::new(stdout))
}

fn leave_terminal(terminal: &mut Tui) -> io::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()
}

fn run_tui(terminal: &mut Tui, setup: Setup) -> io::Result<()> {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let context = WorkerContext {
        api: setup.api,
        bridge: Box::new(herdr::SocketBridge::new()),
        jql: setup.jql,
        owner_pane: setup.owner_pane.clone(),
        worktree_roots: setup.worktree_roots,
        browser_command: setup.browser_command,
    };
    std::thread::Builder::new()
        .name("herdr-jira-worker".into())
        .spawn(move || worker::run(context, job_rx, outcome_tx))?;
    let mut state = JiraState::new(
        setup.project,
        setup.browse_base,
        setup.status_order,
        setup.refresh_seconds,
        setup.owner_pane,
    );
    let send = |jobs: Vec<Job>| {
        for job in jobs {
            let _ = job_tx.send(job);
        }
    };
    send(state.start_refresh().into_iter().collect());
    loop {
        terminal.draw(|frame| ui::render(frame, &mut state, Instant::now()))?;
        if event::poll(POLL)? {
            let effects = match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => state.key(key),
                Event::Mouse(mouse) => state.mouse(mouse),
                Event::Paste(text) => {
                    state.paste(&text);
                    state::Effects::default()
                }
                _ => state::Effects::default(),
            };
            if effects.quit {
                return Ok(());
            }
            send(effects.jobs);
        }
        while let Ok(outcome) = outcome_rx.try_recv() {
            let effects = state.apply(outcome);
            send(effects.jobs);
        }
        send(state.tick(Instant::now()).into_iter().collect());
    }
}

fn run_error_screen(terminal: &mut Tui, problems: &[String]) -> io::Result<()> {
    loop {
        terminal.draw(|frame| {
            let mut lines = vec![
                Line::from(Span::styled(
                    " jira: falta configurar",
                    Style::default().fg(ui::RED).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            lines.extend(
                problems
                    .iter()
                    .map(|problem| Line::from(format!(" • {problem}"))),
            );
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " config: ~/.config/herdr/config.toml · depois: herdr server reload-config · q sai",
                Style::default().fg(ui::GRAY),
            )));
            frame.render_widget(
                Paragraph::new(lines).wrap(Wrap { trim: false }),
                frame.area(),
            );
        })?;
        if event::poll(POLL)? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, event::KeyCode::Char('q') | event::KeyCode::Esc) {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> JiraConfig {
        JiraConfig {
            site: "https://acme.atlassian.net/".into(),
            email: "me@acme.com".into(),
            project: "VK25".into(),
            token_env: "ACME_TOKEN".into(),
            ..JiraConfig::default()
        }
    }

    #[test]
    fn setup_builds_default_jql_and_reads_token_and_owner_from_env() {
        let setup = resolve_setup(&config(), |name| match name {
            "ACME_TOKEN" => Some("tok-value".into()),
            "HERDR_PANEL_OWNER_PANE_ID" => Some("w1:p2".into()),
            _ => None,
        })
        .unwrap_or_else(|problems| panic!("{problems:?}"));

        assert_eq!(
            setup.jql,
            "project = VK25 AND assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC"
        );
        assert_eq!(setup.browse_base, "https://acme.atlassian.net/browse/");
        assert_eq!(setup.owner_pane.as_deref(), Some("w1:p2"));
        assert!(!format!("{:?}", setup.api).contains("tok-value"));
    }

    #[test]
    fn missing_settings_are_listed_without_values() {
        let problems = resolve_setup(&JiraConfig::default(), |_| None)
            .err()
            .unwrap();

        assert_eq!(problems.len(), 4);
        assert!(problems[3].contains("JIRA_API_TOKEN"));
    }

    #[test]
    fn custom_jql_replaces_the_project_query() {
        let mut config = config();
        config.jql = "assignee = currentUser()".into();
        config.project.clear();

        let setup = resolve_setup(&config, |_| Some("tok".into()))
            .unwrap_or_else(|problems| panic!("{problems:?}"));

        assert_eq!(setup.jql, "assignee = currentUser()");
        assert_eq!(setup.project, "jql");
    }
}
