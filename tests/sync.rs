//! End-to-end sync against FCBQ and Google Calendar stubbed over local HTTP.

mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};

use bac_calendar::config::Config;
use bac_calendar::google::CalendarClient;
use bac_calendar::{fcbq, sync};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use support::fake_google::{ACCESS_TOKEN, FakeCalendar};
use support::{FCBQ_ACCESS_DENIED, FCBQ_SEASON, Response, StubServer};

const KEY: &str = "test-key";
const SENIOR: &str = "senior@group.calendar.google.com";
const OTHER: &str = "other@group.calendar.google.com";

struct Harness {
    /// The season FCBQ serves for team 92430, as decoded JSON.
    season: Arc<Mutex<Value>>,
    google: Arc<FakeCalendar>,
    fcbq: fcbq::Client,
    calendar: CalendarClient,
    config: Config,
}

impl Harness {
    fn new() -> Self {
        let decoded = STANDARD.decode(FCBQ_SEASON.trim_ascii()).unwrap();
        let season = Arc::new(Mutex::new(
            serde_json::from_slice::<Value>(&decoded).unwrap(),
        ));
        let served = Arc::clone(&season);
        let fcbq_server = StubServer::start(move |request| {
            match request.path.strip_prefix(&format!("/fcbq/1/{KEY}/")) {
                Some("Match/getByTeamAllSeason/92430") => {
                    let body = served.lock().unwrap().to_string();
                    Response::ok(STANDARD.encode(body))
                }
                Some("Match/getByTeamAllSeason/77777") => Response::ok(
                    STANDARD
                        .encode(r#"{"result":"OK","errorCode":0,"message":"","messageData":[]}"#),
                ),
                _ => Response::ok(FCBQ_ACCESS_DENIED),
            }
        });

        let (google, google_server) = FakeCalendar::serve();
        google.add_calendar(SENIOR);
        google.add_calendar(OTHER);

        let config = Config::parse(
            &format!(
                r#"
                google_client_file = "/unused.json"
                event_duration_minutes = 105

                [[teams]]
                label = "BAC Sènior M"
                fcbq_team_id = 92430
                calendar_id = "{SENIOR}"
                "#
            ),
            Path::new("/"),
        )
        .unwrap();

        Self {
            season,
            google,
            fcbq: fcbq::Client::new(format!("{}/fcbq/1", fcbq_server.base_url), KEY),
            calendar: CalendarClient::new(google_server.base_url, ACCESS_TOKEN),
            config,
        }
    }

    fn sync(&self, dry_run: bool) -> (anyhow::Result<()>, String) {
        let mut out = Vec::new();
        let result = sync::run(&self.config, &self.fcbq, &self.calendar, dry_run, &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    fn sync_ok(&self) -> String {
        let (result, out) = self.sync(false);
        result.unwrap_or_else(|err| panic!("sync failed: {err:#}\n{out}"));
        out
    }

    fn edit_match(&self, index: usize, edit: impl FnOnce(&mut Value)) {
        edit(&mut self.season.lock().unwrap()["messageData"][index]);
    }

    fn managed_event(&self, match_id: &str) -> Value {
        self.google
            .events(SENIOR)
            .into_iter()
            .find(|event| event["extendedProperties"]["private"]["bacCalendarMatchId"] == match_id)
            .unwrap_or_else(|| panic!("no event for match {match_id}"))
    }
}

fn hand_made_event() -> Value {
    json!({
        "id": "hand-made",
        "summary": "Entrenament",
        "start": { "dateTime": "2026-09-24T19:00:00+02:00" },
        "end": { "dateTime": "2026-09-24T20:30:00+02:00" }
    })
}

#[test]
fn the_first_sync_creates_every_match() {
    let harness = Harness::new();

    let out = harness.sync_ok();

    assert_eq!(harness.google.writes().len(), 24, "{out}");
    assert!(
        harness
            .google
            .writes()
            .iter()
            .all(|write| write == &format!("POST {SENIOR}"))
    );
    assert!(out.contains("24 created, 0 updated, 0 deleted"), "{out}");

    let first = harness.managed_event("23379");
    assert_eq!(
        first["summary"],
        "BAC Sènior M vs COOPISTERIA PAPERS CB BELLPUIG B"
    );
    assert_eq!(first["start"]["dateTime"], "2026-09-26T16:00:00Z");
    assert_eq!(first["end"]["dateTime"], "2026-09-26T17:45:00Z");
    assert_eq!(first["start"]["timeZone"], "Europe/Madrid");
    assert_eq!(
        first["location"],
        "PAVELLO MUNICIPAL D'AGRAMUNT, PLAÇA FONDANDANA, S/N, 25310 Agramunt"
    );
    assert_eq!(
        first["extendedProperties"]["private"]["bacCalendarTeamId"],
        "92430"
    );
}

#[test]
fn a_second_sync_plans_nothing() {
    let harness = Harness::new();
    harness.sync_ok();
    harness.google.clear_writes();

    let out = harness.sync_ok();

    assert_eq!(harness.google.writes(), Vec::<String>::new());
    assert!(out.contains("up to date (24 matches)"), "{out}");
}

#[test]
fn changes_in_the_schedule_are_carried_over() {
    let harness = Harness::new();
    harness.sync_ok();
    harness.google.clear_writes();
    let moved = harness.managed_event("23395")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dropped = harness.managed_event("23401")["id"]
        .as_str()
        .unwrap()
        .to_owned();

    harness.edit_match(1, |record| {
        record["matchDay"] = json!("2026-10-11 12:00:00");
        record["idMatchChangePetition"] = json!("4321");
    });
    harness.edit_match(3, |record| {
        record["nameField"] = json!("PAVELLO NOU DE TÀRREGA")
    });
    harness.season.lock().unwrap()["messageData"]
        .as_array_mut()
        .unwrap()
        .remove(2);
    let out = harness.sync_ok();

    assert!(out.contains("0 created, 2 updated, 1 deleted"), "{out}");
    assert!(out.contains("(time, details)"), "{out}");
    assert!(out.contains("(venue)"), "{out}");
    assert!(out.contains("(no longer scheduled)"), "{out}");
    assert_eq!(harness.google.writes().len(), 3);

    let updated = harness.google.event(SENIOR, &moved).unwrap();
    assert_eq!(updated["start"]["dateTime"], "2026-10-11T10:00:00Z");
    assert!(
        updated["description"]
            .as_str()
            .unwrap()
            .contains("canvi d'horari pendent")
    );
    assert_eq!(
        updated["extendedProperties"]["private"]["bacCalendarMatchId"],
        "23395"
    );
    assert_eq!(harness.google.event(SENIOR, &dropped), None);

    harness.google.clear_writes();
    harness.sync_ok();
    assert_eq!(harness.google.writes(), Vec::<String>::new());
}

#[test]
fn events_the_tool_does_not_own_are_never_touched() {
    let harness = Harness::new();
    let foreign_marker = json!({
        "id": "other-team",
        "summary": "Another team's match",
        "extendedProperties": { "private": {
            "bacCalendarTeamId": "11111",
            "bacCalendarMatchId": "23379"
        } }
    });
    harness.google.seed(SENIOR, hand_made_event());
    harness.google.seed(SENIOR, foreign_marker.clone());
    harness.google.seed(OTHER, hand_made_event());

    harness.sync_ok();
    harness.season.lock().unwrap()["messageData"] = json!([{ "idMatch": "1" }]);
    harness.sync_ok();

    assert_eq!(
        harness.google.event(SENIOR, "hand-made"),
        Some(hand_made_event())
    );
    assert_eq!(
        harness.google.event(SENIOR, "other-team"),
        Some(foreign_marker)
    );
    assert_eq!(harness.google.events(OTHER), [hand_made_event()]);
    assert!(
        harness
            .google
            .writes()
            .iter()
            .all(|write| !write.contains("hand-made")
                && !write.contains("other-team")
                && !write.contains(OTHER))
    );
    assert_eq!(
        harness.google.events(SENIOR).len(),
        2,
        "every managed event was removed"
    );
}

#[test]
fn a_dry_run_prints_the_plan_and_writes_nothing() {
    let harness = Harness::new();

    let (result, out) = harness.sync(true);

    result.unwrap();
    assert_eq!(harness.google.writes(), Vec::<String>::new());
    assert!(out.contains("BAC Sènior M (FCBQ team 92430) -> senior@group.calendar.google.com"));
    assert!(
        out.contains("  create  2026-09-26 18:00  BAC Sènior M vs COOPISTERIA"),
        "{out}"
    );
    assert!(
        out.contains("dry run: 24 to create, 0 to update, 0 to delete"),
        "{out}"
    );
}

#[test]
fn an_empty_schedule_keeps_the_existing_events() {
    let mut harness = Harness::new();
    harness.sync_ok();
    harness.google.clear_writes();
    harness.config.teams[0].fcbq_team_id = 77777;
    harness.google.seed(
        SENIOR,
        json!({
            "id": "stale",
            "summary": "Last season",
            "extendedProperties": { "private": {
                "bacCalendarTeamId": "77777",
                "bacCalendarMatchId": "1"
            } }
        }),
    );

    let out = harness.sync_ok();

    assert!(
        out.contains("FCBQ returned no matches for team 77777"),
        "{out}"
    );
    assert_eq!(harness.google.writes(), Vec::<String>::new());
}

#[test]
fn a_failing_team_does_not_stop_the_others() {
    let mut harness = Harness::new();
    let mut broken = harness.config.teams[0].clone();
    broken.label = "Broken".into();
    broken.fcbq_team_id = 123;
    broken.calendar_id = OTHER.into();
    harness.config.teams.insert(0, broken);

    let (result, out) = harness.sync(false);

    let err = result.unwrap_err();
    assert_eq!(err.to_string(), "1 of 2 teams failed: Broken");
    assert!(
        out.contains("error: fetching the FCBQ schedule: FCBQ denied access"),
        "{out}"
    );
    assert!(out.contains("24 created"), "{out}");
}
