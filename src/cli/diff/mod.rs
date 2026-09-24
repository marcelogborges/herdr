pub(crate) mod ansi;
pub(crate) mod git;
pub(crate) mod state;
pub(crate) mod ui;
pub(crate) mod worker;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use self::state::{DiffState, Effects, External};
use self::worker::{Job, WorkerContext};

const POLL: Duration = Duration::from_millis(150);
const OWNER_PANE_ENVS: [&str; 2] = ["HERDR_PANEL_OWNER_PANE_ID", "HERDR_ACTIVE_PANE_ID"];

type Tui = Terminal<CrosstermBackend<Stdout>>;

pub(super) fn run_diff_command(args: &[String]) -> io::Result<i32> {
    if let Some(arg) = args.first() {
        if matches!(arg.as_str(), "help" | "--help" | "-h") {
            print_help();
            return Ok(0);
        }
        eprintln!("unexpected argument: {arg}");
        print_help();
        return Ok(2);
    }
    let owner_pane = OWNER_PANE_ENVS.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
    });
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut terminal = enter_terminal()?;
    let result = run_tui(&mut terminal, owner_pane, cwd, home);
    leave_terminal(&mut terminal)?;
    result.map(|()| 0)
}

fn print_help() {
    eprintln!("herdr diff: changes of every repo the pane's Claude session touched");
    eprintln!("  usage: herdr diff");
    eprintln!("  keys: enter open · b PR/uncommitted · e edit · g lazygit · r refresh · q quit");
}

fn enter_terminal() -> io::Result<Tui> {
    resume_terminal()?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = suspend_terminal();
        previous(info);
    }));
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

fn resume_terminal() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
}

fn suspend_terminal() -> io::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen)
}

fn leave_terminal(terminal: &mut Tui) -> io::Result<()> {
    suspend_terminal()?;
    terminal.show_cursor()
}

fn run_external(terminal: &mut Tui, external: &External) -> io::Result<Result<(), String>> {
    suspend_terminal()?;
    terminal.show_cursor()?;
    let status = std::process::Command::new(&external.program)
        .args(&external.args)
        .current_dir(&external.dir)
        .status();
    resume_terminal()?;
    terminal.clear()?;
    Ok(match status {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("{}: {err}", external.program)),
    })
}

fn run_tui(
    terminal: &mut Tui,
    owner_pane: Option<String>,
    cwd: PathBuf,
    home: Option<PathBuf>,
) -> io::Result<()> {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let context = WorkerContext {
        bridge: Box::new(crate::cli::jira::herdr::SocketBridge::new()),
        owner_pane,
        cwd: cwd.clone(),
    };
    std::thread::Builder::new()
        .name("herdr-diff-worker".into())
        .spawn(move || worker::run(context, job_rx, outcome_tx))?;
    let mut state = DiffState::new(cwd, home);
    let send = |jobs: Vec<Job>| {
        for job in jobs {
            let _ = job_tx.send(job);
        }
    };
    send(state.start_refresh().into_iter().collect());
    loop {
        terminal.draw(|frame| ui::render(frame, &mut state))?;
        let mut pending = Vec::new();
        if event::poll(POLL)? {
            pending.push(match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => state.key(key),
                Event::Mouse(mouse) => state.mouse(mouse),
                _ => Effects::default(),
            });
        }
        while let Ok(outcome) = outcome_rx.try_recv() {
            pending.push(state.apply(outcome));
        }
        for effects in pending {
            if effects.quit {
                return Ok(());
            }
            send(effects.jobs);
            if let Some(external) = effects.external {
                let result = run_external(terminal, &external)?;
                send(state.after_external(result).jobs);
            }
        }
        send(state.tick(Instant::now()).into_iter().collect());
    }
}
