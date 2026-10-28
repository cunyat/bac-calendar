//! The Calendar client and OAuth token refresh against local HTTP stubs.

mod support;

use std::sync::{Arc, Mutex};

use bac_calendar::google::oauth::{self, ClientSecrets};
use bac_calendar::google::{CalendarClient, Event};
use serde_json::json;
use support::fake_google::{ACCESS_TOKEN, FakeCalendar};
use support::{Response, StubServer};

#[test]
fn lists_every_calendar_across_pages() {
    let (google, server) = FakeCalendar::serve();
    for id in [
        "a@group.calendar.google.com",
        "b@group.calendar.google.com",
        "me@gmail.com",
    ] {
        google.add_calendar(id);
    }

    let calendars = CalendarClient::new(server.base_url, ACCESS_TOKEN)
        .calendars()
        .unwrap();

    let ids: Vec<_> = calendars.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "a@group.calendar.google.com",
            "b@group.calendar.google.com",
            "me@gmail.com"
        ]
    );
    assert_eq!(calendars[0].name(), "Calendar a@group.calendar.google.com");
    assert_eq!(calendars[0].access_role.as_deref(), Some("owner"));
}

#[test]
fn lists_events_by_private_property_across_pages() {
    let (google, server) = FakeCalendar::serve();
    for n in 0..25 {
        google.seed(
            "cal",
            json!({ "id": format!("m{n}"), "extendedProperties": { "private": { "team": "1" } } }),
        );
    }
    google.seed("cal", json!({ "id": "unmarked" }));

    let events = CalendarClient::new(server.base_url, ACCESS_TOKEN)
        .events_with_private_property("cal", "team", "1")
        .unwrap();

    assert_eq!(events.len(), 25);
    assert!(
        events
            .iter()
            .all(|event| event.private_property("team") == Some("1"))
    );
}

#[test]
fn deleting_an_event_that_is_already_gone_succeeds() {
    let (google, server) = FakeCalendar::serve();
    google.add_calendar("cal");

    CalendarClient::new(server.base_url, ACCESS_TOKEN)
        .delete_event("cal", "gone")
        .unwrap();
}

#[test]
fn reports_google_error_messages() {
    let (google, server) = FakeCalendar::serve();
    google.add_calendar("cal");

    let err = CalendarClient::new(server.base_url, "expired-token")
        .insert_event("cal", &Event::default())
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "creating an event failed with HTTP 401: Request had invalid authentication credentials."
    );
}

#[test]
fn refreshes_the_access_token_with_the_stored_refresh_token() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let server = StubServer::start(move |request| {
        recorded.lock().unwrap().extend(request.form());
        Response::json(200, &json!({ "access_token": "fresh", "expires_in": 3599 }))
    });
    let token_path = temp_dir("refresh").join("google-token.json");
    oauth::save_refresh_token(&token_path, "stored-refresh").unwrap();

    let token = oauth::access_token(&secrets(&format!("{}/token", server.base_url)), &token_path);

    assert_eq!(token.unwrap(), "fresh");
    let mut form = seen.lock().unwrap().clone();
    form.sort();
    assert_eq!(
        form,
        [
            (
                "client_id".to_owned(),
                "id.apps.googleusercontent.com".to_owned()
            ),
            ("client_secret".to_owned(), "shh".to_owned()),
            ("grant_type".to_owned(), "refresh_token".to_owned()),
            ("refresh_token".to_owned(), "stored-refresh".to_owned()),
        ]
    );
}

#[test]
fn a_revoked_refresh_token_asks_to_sign_in_again() {
    let server = StubServer::start(|_| {
        Response::json(
            400,
            &json!({ "error": "invalid_grant", "error_description": "Token has been expired or revoked." }),
        )
    });
    let token_path = temp_dir("revoked").join("google-token.json");
    oauth::save_refresh_token(&token_path, "revoked").unwrap();

    let err = oauth::access_token(&secrets(&server.base_url), &token_path).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("run `bac-calendar auth` again"),
        "{message}"
    );
    assert!(
        message.contains("HTTP 400: Token has been expired or revoked."),
        "{message}"
    );
}

#[test]
fn a_missing_token_file_asks_to_sign_in() {
    let err = oauth::access_token(
        &secrets("http://127.0.0.1:9"),
        &temp_dir("none").join("t.json"),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("run `bac-calendar auth` first"),
        "{err}"
    );
}

fn secrets(token_uri: &str) -> ClientSecrets {
    ClientSecrets {
        client_id: "id.apps.googleusercontent.com".into(),
        client_secret: "shh".into(),
        auth_uri: "https://accounts.google.com/o/oauth2/auth".into(),
        token_uri: token_uri.into(),
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bac-calendar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
