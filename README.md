# bac-calendar

Keeps your basketball teams' matches in your Google calendars. For every
configured team, `bac-calendar sync` reads the team's season schedule from the
Catalan basketball federation (FCBQ) and makes the team's Google calendar match
it: missing matches are created, rescheduled ones updated, and matches that left
the schedule removed.

The sync is idempotent and safe in calendars you also edit by hand. Every event
it creates carries two private markers, the FCBQ team id and match id, and it
only ever updates or deletes events carrying the marker of the team being
synced. Your own events are never read into a sync plan, so they are never
touched.

Each match becomes an event in the `Europe/Madrid` time zone with:

- a title in `Home vs Away` order, with your team shown by its configured label;
- the venue name and address as the location;
- a description with the category, competition, group and round, whether it is a
  home or away match, a link to the match on basquetcatala.cat, and a warning
  while a schedule change petition is pending.

Matches are read from the backend basquetcatala.cat itself uses.

## Build

```sh
nix build                    # result/bin/bac-calendar
nix run . -- sync --dry-run  # one-off run without installing
cargo build --release        # target/release/bac-calendar
```

Run it without cloning with `nix run github:cunyat/bac-calendar -- <command>`.
The [Home Manager section](#run-it-periodically-with-home-manager) installs and
schedules it declaratively instead.

## Set up Google access

The tool signs in with an OAuth client that lives in your own Google Cloud
project, so no third party ever holds access to your calendars.

1. Open the [Google Cloud console](https://console.cloud.google.com/) and create
   a project, for example `bac-calendar`.
2. In **APIs & Services > Library**, find **Google Calendar API** and click
   **Enable**.
3. In **Google Auth Platform > Branding**, fill in the app name and your email.
4. In **Google Auth Platform > Audience**, choose **External** and click
   **Publish app** so the status reads **In production**. In **Testing** status,
   Google expires refresh tokens after 7 days and the timer would stop working
   every week. The app is never submitted for verification, so the consent
   screen will warn that Google has not verified it; continue past the warning,
   since you are its only user.
5. In **Google Auth Platform > Clients**, click **Create client**, choose
   application type **Desktop app**, and download the client JSON.
6. Save it as `~/.config/bac-calendar/client_secret.json`.

The tool requests two scopes:

- `calendar.calendarlist.readonly` to list your calendars and their ids;
- `calendar.events.owned` to manage events on calendars you own.

Calendars shared with you by someone else cannot be synced; `calendars` shows
your access role on each one.

## Get the FCBQ access key

The FCBQ backend takes the access key that basquetcatala.cat ships to every
browser. Open basquetcatala.cat with the browser's developer tools on the
Network tab: requests to `esb.optimalwayconsulting.com/fcbq/1/<KEY>/...` carry
it as the path segment after `/fcbq/1/`. The federation can rotate it at any
time; when it does, `sync` fails with `FCBQ denied access` until you replace it.

Keep it out of any repository. Either export it as `FCBQ_API_KEY`, or store it in
a file readable only by you and name that file in the config:

```sh
install -m 600 /dev/null ~/.config/bac-calendar/fcbq-api-key
$EDITOR ~/.config/bac-calendar/fcbq-api-key
```

`FCBQ_API_KEY` wins over the key file when both are set.

## Configure

`~/.config/bac-calendar/config.toml` (`$XDG_CONFIG_HOME` is honoured; `--config`
points elsewhere):

```toml
# Relative paths are relative to this file; ~/ is your home.
google_client_file = "client_secret.json"
fcbq_api_key_file = "fcbq-api-key"

# Event length, in minutes. Defaults to 120.
event_duration_minutes = 120

# One [[teams]] block per team.
[[teams]]
label = "BAC Sènior M"
fcbq_team_id = 92430
calendar_id = "0123456789abcdef@group.calendar.google.com"
```

`label` is how the team appears in event titles. Fill `fcbq_team_id` from
`bac-calendar teams <club-id>` and `calendar_id` from `bac-calendar calendars`.

FCBQ team ids change every season. When a new season's schedule is published,
look the teams up again and update the ids. Events created under the old id stay
in the calendar as history, and a team id whose schedule comes back empty never
deletes anything: `sync` warns instead.

## Commands

```text
bac-calendar auth               Sign in to Google once and store the refresh token
bac-calendar calendars          List your Google calendars and their ids
bac-calendar teams <club-id>    List a club's teams with this season's FCBQ team ids
bac-calendar sync [--dry-run]   Sync every configured team; --dry-run only prints the plan
```

`auth` prints a Google consent URL and opens it in your browser; Google then
redirects back to a one-shot listener on `127.0.0.1`. The refresh token is
stored at `~/.local/share/bac-calendar/google-token.json` (`$XDG_DATA_HOME` is
honoured), readable only by you. Run `auth` again if Google revokes it.

`teams` takes a club id, the number in the club's basquetcatala.cat URL: 393 is
Bàsquet Agramunt Club and 403 is Club Natació Tàrrega.

`sync` prints, per team, every create, update (with what changed: title, time,
venue or details) and delete, then applies them. A team that fails does not stop
the others, and the command exits non-zero if any team failed.

```text
$ bac-calendar sync --dry-run
BAC Sènior M (FCBQ team 92430) -> 0123456789abcdef@group.calendar.google.com
  update  2026-11-07 19:00  BAC Sènior M vs C.B. TORREFARRERA A (time, details)
  dry run: 0 to create, 1 to update, 0 to delete
```

## Run it periodically with Home Manager

The flake ships a Home Manager module that installs the tool and runs
`bac-calendar sync` from a systemd user timer. Add the flake as an input of the
flake that holds your Home Manager configuration:

```nix
inputs.bac-calendar = {
  url = "github:cunyat/bac-calendar";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Then, with `inputs` passed to Home Manager through `extraSpecialArgs`, import the
module and enable it:

```nix
{inputs, ...}: {
  imports = [inputs.bac-calendar.homeManagerModules.default];

  services.bac-calendar.enable = true;
}
```

It syncs every three hours by default; set `services.bac-calendar.schedule` to
any systemd calendar expression to change that. `Persistent=true` runs a sync
missed while the machine was off as soon as it is back. The module leaves the
config file, Google client, FCBQ key and token to you: set them up as described
above and run `bac-calendar auth` once by hand before the first timer fires.

Pick up new commits with `nix flake update bac-calendar`, and check runs with
`journalctl --user -u bac-calendar`.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
nix fmt -- --check .
nix flake check
```

The tests run against captured FCBQ responses in
[tests/fixtures/](tests/fixtures/) and against FCBQ and Google Calendar stubbed
over local HTTP; none of them reach the network.

## License

Released under the [MIT License](LICENSE).
