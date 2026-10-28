use std::path::PathBuf;

use anyhow::{Context, Result, bail};

pub const USAGE: &str = "\
Syncs your basketball teams' FCBQ matches into their Google calendars.

Usage: bac-calendar [--config <path>] <command>

Commands:
  auth               Sign in to Google once and store the refresh token
  calendars          List your Google calendars and their ids
  teams <club-id>    List a club's teams with this season's FCBQ team ids
  sync [--dry-run]   Sync every configured team; --dry-run only prints the plan

Options:
  --config <path>    Config file [default: $XDG_CONFIG_HOME/bac-calendar/config.toml]
  -h, --help         Print this help
  -V, --version      Print the version

FCBQ_API_KEY overrides the config's fcbq_api_key_file.";

#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub config: Option<PathBuf>,
    pub command: Command,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Auth,
    Calendars,
    Teams { club_id: u64 },
    Sync { dry_run: bool },
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Invocation> {
    let mut config = None;
    let mut words = Vec::new();
    let mut dry_run = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Ok(Invocation {
                    config,
                    command: Command::Help,
                });
            }
            "-V" | "--version" => {
                return Ok(Invocation {
                    config,
                    command: Command::Version,
                });
            }
            "--config" => {
                config = Some(PathBuf::from(args.next().context("--config needs a path")?))
            }
            "--dry-run" => dry_run = true,
            flag if flag.starts_with('-') => bail!("unknown option {flag}"),
            _ => words.push(arg),
        }
    }

    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let command = match words.as_slice() {
        [] | ["help"] => Command::Help,
        ["auth"] => Command::Auth,
        ["calendars"] => Command::Calendars,
        ["teams", club_id] => Command::Teams {
            club_id: club_id
                .parse()
                .with_context(|| format!("invalid club id {club_id}"))?,
        },
        ["teams"] => bail!("teams needs a club id, e.g. `bac-calendar teams 393`"),
        ["sync"] => Command::Sync { dry_run },
        [
            command @ ("auth" | "calendars" | "teams" | "sync" | "help"),
            ..,
        ] => {
            bail!("too many arguments for {command}")
        }
        [command, ..] => bail!("unknown command {command}"),
    };
    if dry_run && !matches!(command, Command::Sync { .. }) {
        bail!("--dry-run only applies to sync");
    }
    Ok(Invocation { config, command })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(line: &str) -> Result<Invocation> {
        parse(line.split_whitespace().map(str::to_owned))
    }

    fn command(line: &str) -> Command {
        parse_str(line).unwrap().command
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(command(""), Command::Help);
        assert_eq!(command("auth"), Command::Auth);
        assert_eq!(command("calendars"), Command::Calendars);
        assert_eq!(command("teams 393"), Command::Teams { club_id: 393 });
        assert_eq!(command("sync"), Command::Sync { dry_run: false });
        assert_eq!(command("sync --dry-run"), Command::Sync { dry_run: true });
        assert_eq!(command("--dry-run sync"), Command::Sync { dry_run: true });
        assert_eq!(command("sync --help"), Command::Help);
        assert_eq!(command("-V"), Command::Version);
    }

    #[test]
    fn takes_a_config_path_anywhere() {
        let invocation = parse_str("sync --config /tmp/c.toml").unwrap();
        assert_eq!(invocation.config, Some(PathBuf::from("/tmp/c.toml")));
        assert_eq!(invocation.command, Command::Sync { dry_run: false });
    }

    #[test]
    fn rejects_bad_invocations() {
        for line in [
            "teams",
            "teams abc",
            "auth --dry-run",
            "sync --force",
            "frobnicate",
            "--config",
        ] {
            assert!(parse_str(line).is_err(), "{line} should be rejected");
        }
    }
}
