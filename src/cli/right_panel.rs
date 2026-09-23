use crate::api::schema::{EmptyParams, Method, Request, RightPanelOpenParams};

pub(super) fn run_right_panel_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_right_panel_help();
        return Ok(2);
    };

    match subcommand {
        "open" => right_panel_open(&args[1..]),
        "toggle" if args.len() == 1 => super::print_response(&super::send_request(&Request {
            id: "cli:right_panel:toggle".into(),
            method: Method::RightPanelToggle(EmptyParams {}),
        })?),
        "help" | "--help" | "-h" => {
            print_right_panel_help();
            Ok(0)
        }
        _ => {
            print_right_panel_help();
            Ok(2)
        }
    }
}

fn right_panel_open(args: &[String]) -> std::io::Result<i32> {
    let cwd = std::env::current_dir().ok();
    let pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|id| !id.is_empty());
    let params = match parse_right_panel_open_args(args, cwd.as_deref(), pane_id) {
        Ok(params) => params,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("usage: herdr right-panel open <PATH> [--line N]");
            return Ok(2);
        }
    };

    super::print_response(&super::send_request(&Request {
        id: "cli:right_panel:open".into(),
        method: Method::RightPanelOpen(params),
    })?)
}

fn parse_right_panel_open_args(
    args: &[String],
    cwd: Option<&std::path::Path>,
    pane_id: Option<String>,
) -> Result<RightPanelOpenParams, String> {
    let mut path = None;
    let mut line = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--line" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --line".to_owned())?;
                line = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid line: {value}"))?,
                );
                index += 2;
            }
            other if other.starts_with("--") => return Err(format!("unknown option: {other}")),
            other => {
                if path.replace(other.to_owned()).is_some() {
                    return Err(format!("unexpected argument: {other}"));
                }
                index += 1;
            }
        }
    }
    let path = path.ok_or_else(|| "missing path".to_owned())?;
    let path = match cwd {
        Some(cwd) if !std::path::Path::new(&path).is_absolute() => {
            cwd.join(&path).to_string_lossy().into_owned()
        }
        _ => path,
    };
    Ok(RightPanelOpenParams {
        path,
        line,
        pane_id,
    })
}

fn print_right_panel_help() {
    eprintln!("herdr right-panel commands:");
    eprintln!("  herdr right-panel open <PATH> [--line N]   open a file (or PATH:LINE) in the right panel");
    eprintln!("  herdr right-panel toggle                   show or hide the right panel");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn open_args_make_relative_paths_absolute_and_keep_line() {
        let params = parse_right_panel_open_args(
            &args(&["src/a.rs", "--line", "42"]),
            Some(std::path::Path::new("/repo")),
            Some("w1:p2".into()),
        )
        .unwrap();

        assert_eq!(
            params,
            RightPanelOpenParams {
                path: "/repo/src/a.rs".into(),
                line: Some(42),
                pane_id: Some("w1:p2".into()),
            }
        );
    }

    #[test]
    fn open_args_keep_absolute_paths_and_line_shorthand_for_the_server() {
        let params =
            parse_right_panel_open_args(&args(&["/abs/a.rs:7"]), Some("/repo".as_ref()), None)
                .unwrap();

        assert_eq!(params.path, "/abs/a.rs:7");
        assert_eq!(params.line, None);
        assert_eq!(params.pane_id, None);
    }

    #[test]
    fn open_args_reject_bad_input() {
        let cwd = Some(std::path::Path::new("/repo"));
        assert!(parse_right_panel_open_args(&args(&[]), cwd, None).is_err());
        assert!(parse_right_panel_open_args(&args(&["a", "b"]), cwd, None).is_err());
        assert!(parse_right_panel_open_args(&args(&["a", "--line"]), cwd, None).is_err());
        assert!(parse_right_panel_open_args(&args(&["a", "--line", "x"]), cwd, None).is_err());
        assert!(parse_right_panel_open_args(&args(&["a", "--col", "1"]), cwd, None).is_err());
    }
}
