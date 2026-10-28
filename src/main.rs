mod cli;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, anyhow, bail};
use bac_calendar::config::{self, Config};
use bac_calendar::google::oauth::{self, ClientSecrets};
use bac_calendar::google::{self, CalendarClient};
use bac_calendar::{fcbq, sync};

use cli::{Command, USAGE};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let invocation = cli::parse(std::env::args().skip(1))
        .map_err(|err| anyhow!("{err:#}; run `bac-calendar --help` for usage"))?;
    let config_path = match invocation.config {
        Some(path) => path,
        None => config::default_config_path()?,
    };
    match invocation.command {
        Command::Help => println!("{USAGE}"),
        Command::Version => println!("bac-calendar {}", env!("CARGO_PKG_VERSION")),
        Command::Auth => auth(&config_path)?,
        Command::Calendars => calendars(&config_path)?,
        Command::Teams { club_id } => teams(&config_path, club_id)?,
        Command::Sync { dry_run } => sync(&config_path, dry_run)?,
    }
    Ok(())
}

fn auth(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let token_path = config::default_token_path()?;
    oauth::sign_in(
        &ClientSecrets::load(&config.google_client_file)?,
        &token_path,
    )?;
    println!(
        "Signed in. The refresh token is stored at {}",
        token_path.display()
    );
    Ok(())
}

fn calendars(config_path: &Path) -> Result<()> {
    let calendar = calendar_client(&Config::load(config_path)?)?;
    let mut entries = calendar.calendars()?;
    entries.sort_by(|a, b| {
        b.primary
            .cmp(&a.primary)
            .then_with(|| a.name().cmp(b.name()))
    });

    let mut rows = vec![[
        "ACCESS".to_owned(),
        "NAME".to_owned(),
        "CALENDAR ID".to_owned(),
    ]];
    rows.extend(entries.iter().map(|entry| {
        [
            entry.access_role.clone().unwrap_or_default(),
            entry.name().to_owned(),
            entry.id.clone(),
        ]
    }));
    print!("{}", table(&rows));
    println!("\nOnly calendars you own (access \"owner\") can be synced.");
    Ok(())
}

fn teams(config_path: &Path, club_id: u64) -> Result<()> {
    let env_key = std::env::var(fcbq::API_KEY_ENV).ok();
    let config = match env_key.as_deref().map(str::trim) {
        Some(key) if !key.is_empty() => None,
        _ => Some(Config::load(config_path)?),
    };
    let api_key = config::fcbq_api_key(env_key, config.as_ref())?;
    let club = fcbq::Client::new(fcbq::DEFAULT_BASE_URL, api_key).club(club_id)?;

    let mut teams: Vec<_> = club
        .categories
        .iter()
        .flat_map(|category| category.teams.iter().map(move |team| (category, team)))
        .collect();
    if teams.is_empty() {
        bail!("FCBQ lists no teams for club {club_id}");
    }
    teams.sort_by(|(_, a), (_, b)| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.name.cmp(&b.name))
    });

    let name = club.name.as_deref().unwrap_or("Unnamed club");
    let seasons: std::collections::BTreeSet<_> = teams
        .iter()
        .filter_map(|(_, team)| team.season.as_deref())
        .collect();
    let seasons = seasons.into_iter().collect::<Vec<_>>().join(", ");
    println!("{name} (club {club_id}), season {seasons}\n");

    let mut rows = vec![[
        "TEAM ID".to_owned(),
        "CATEGORY".to_owned(),
        "TEAM".to_owned(),
    ]];
    rows.extend(teams.iter().map(|(category, team)| {
        [
            team.id.clone(),
            team.category
                .clone()
                .or_else(|| category.name.clone())
                .unwrap_or_default(),
            team.name.clone().unwrap_or_default(),
        ]
    }));
    print!("{}", table(&rows));
    Ok(())
}

fn sync(config_path: &Path, dry_run: bool) -> Result<()> {
    let config = Config::load(config_path)?;
    if config.teams.is_empty() {
        bail!(
            "no teams in {}; add a [[teams]] entry first",
            config_path.display()
        );
    }
    let api_key = config::fcbq_api_key(std::env::var(fcbq::API_KEY_ENV).ok(), Some(&config))?;
    let fcbq = fcbq::Client::new(fcbq::DEFAULT_BASE_URL, api_key);
    let calendar = calendar_client(&config)?;
    sync::run(
        &config,
        &fcbq,
        &calendar,
        dry_run,
        &mut std::io::stdout().lock(),
    )
}

fn calendar_client(config: &Config) -> Result<CalendarClient> {
    let secrets = ClientSecrets::load(&config.google_client_file)?;
    let token_path: PathBuf = config::default_token_path()?;
    let access_token = oauth::access_token(&secrets, &token_path)?;
    Ok(CalendarClient::new(google::DEFAULT_BASE_URL, access_token))
}

/// Left-aligned columns separated by two spaces.
fn table<const N: usize>(rows: &[[String; N]]) -> String {
    let mut widths = [0; N];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell}{}", " ".repeat(width - cell.chars().count())))
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}
