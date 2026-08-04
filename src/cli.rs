use crate::click::{cps_error, valid_cps, DEFAULT_CPS, MAX_CPS, MIN_CPS};
use clap::{CommandFactory, Parser};

const HELP: &str = "EC generates repeated left mouse clicks.

Usage:
  ec start
  ec <CPS>
  ec stop
  ec status
  ec setup
  ec doctor

Commands:
  start     Start EC at 20 CPS
  stop      Stop EC
  status    Show EC status
  setup     Configure device access and mouse selection
  doctor    Check system readiness

Arguments:
  <CPS>     Start EC at 1–600 CPS

Examples:
  ec start
  ec 100
  ec 600
  ec stop
  ec status
  ec setup
  ec doctor";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Start(u32),
    Stop,
    Status,
    Setup,
    Doctor,
}

fn parse_action(value: &str) -> Result<Action, String> {
    match value {
        "start" => Ok(Action::Start(DEFAULT_CPS)),
        "stop" => Ok(Action::Stop),
        "status" => Ok(Action::Status),
        "setup" => Ok(Action::Setup),
        "doctor" => Ok(Action::Doctor),
        _ => {
            let cps = value.parse::<u32>().map_err(|_| {
                format!(
                    "unknown command or CPS `{value}`; expected start, stop, status, setup, doctor, or {MIN_CPS}–{MAX_CPS}"
                )
            })?;
            if valid_cps(cps) {
                Ok(Action::Start(cps))
            } else {
                Err(cps_error())
            }
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "ec",
    disable_version_flag = true,
    disable_help_subcommand = true,
    override_help = HELP
)]
pub struct Cli {
    #[arg(value_parser = parse_action)]
    pub action: Option<Action>,
}

pub fn print_help() -> std::io::Result<()> {
    Cli::command().print_help()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Action>, clap::Error> {
        Cli::try_parse_from(args).map(|cli| cli.action)
    }

    #[test]
    fn accepts_exact_operational_forms() {
        assert_eq!(parse(&["ec"]).unwrap(), None);
        assert_eq!(parse(&["ec", "start"]).unwrap(), Some(Action::Start(20)));
        for cps in [1, 20, 100, 600] {
            assert_eq!(
                parse(&["ec", &cps.to_string()]).unwrap(),
                Some(Action::Start(cps))
            );
        }
        assert_eq!(parse(&["ec", "stop"]).unwrap(), Some(Action::Stop));
        assert_eq!(parse(&["ec", "status"]).unwrap(), Some(Action::Status));
        assert_eq!(parse(&["ec", "setup"]).unwrap(), Some(Action::Setup));
        assert_eq!(parse(&["ec", "doctor"]).unwrap(), Some(Action::Doctor));
    }

    #[test]
    fn accepts_only_standard_help_options() {
        for help in ["--help", "-h"] {
            assert_eq!(
                parse(&["ec", help]).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
    }

    #[test]
    fn rejects_removed_interface_and_invalid_input() {
        let cases: &[&[&str]] = &[
            &["ec", "test"],
            &["ec", "config"],
            &["ec", "start", "600"],
            &["ec", "start", "--cps", "600"],
            &["ec", "start", "--burst", "3"],
            &["ec", "start", "--background"],
            &["ec", "start", "--button", "left"],
            &["ec", "start", "--device", "/dev/input/event11"],
            &["ec", "start", "--interval", "20"],
            &["ec", "start", "--verbose"],
            &["ec", "doctor", "--verbose"],
            &["ec", "setup", "extra"],
            &["ec", "--version"],
            &["ec", "0"],
            &["ec", "601"],
            &["ec", "abc"],
            &["ec", "100", "extra"],
        ];
        for args in cases {
            assert!(parse(args).is_err(), "unexpectedly accepted {args:?}");
        }
    }

    #[test]
    fn help_contains_only_the_minimal_surface() {
        let mut help = Vec::new();
        Cli::command().write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();
        for accepted in [
            "ec start",
            "ec <CPS>",
            "ec stop",
            "ec status",
            "ec setup",
            "ec doctor",
        ] {
            assert!(help.contains(accepted));
        }
        for removed in [
            "test",
            "config",
            "burst",
            "background",
            "verbose",
            "version",
        ] {
            assert!(!help.contains(removed));
        }
    }
}
