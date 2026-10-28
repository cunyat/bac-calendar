//! An in-memory stand-in for the Calendar API v3 endpoints the tool uses.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use super::{Request, Response, StubServer};

pub const ACCESS_TOKEN: &str = "test-access-token";

/// Page size for event listings, small enough to exercise pagination.
const PAGE_SIZE: usize = 10;

#[derive(Default)]
pub struct FakeCalendar {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    calendars: BTreeMap<String, Vec<Value>>,
    next_id: usize,
    writes: Vec<String>,
}

impl FakeCalendar {
    pub fn serve() -> (Arc<Self>, StubServer) {
        let fake = Arc::new(Self::default());
        let handler = Arc::clone(&fake);
        let server = StubServer::start(move |request| handler.handle(request));
        (fake, server)
    }

    pub fn add_calendar(&self, calendar_id: &str) {
        let mut state = self.state.lock().unwrap();
        state.calendars.entry(calendar_id.to_owned()).or_default();
    }

    /// Adds an event as if the user had created it by hand.
    pub fn seed(&self, calendar_id: &str, event: Value) {
        let mut state = self.state.lock().unwrap();
        state
            .calendars
            .entry(calendar_id.to_owned())
            .or_default()
            .push(event);
    }

    pub fn events(&self, calendar_id: &str) -> Vec<Value> {
        let state = self.state.lock().unwrap();
        state
            .calendars
            .get(calendar_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn event(&self, calendar_id: &str, event_id: &str) -> Option<Value> {
        self.events(calendar_id)
            .into_iter()
            .find(|event| event["id"] == event_id)
    }

    /// Every write served so far, as `METHOD calendar [event]`.
    pub fn writes(&self) -> Vec<String> {
        self.state.lock().unwrap().writes.clone()
    }

    pub fn clear_writes(&self) {
        self.state.lock().unwrap().writes.clear();
    }

    fn handle(&self, request: &Request) -> Response {
        if request.header("authorization") != Some(&format!("Bearer {ACCESS_TOKEN}")) {
            return error(401, "Request had invalid authentication credentials.");
        }
        let segments: Vec<&str> = request.path.trim_start_matches('/').split('/').collect();
        let mut state = self.state.lock().unwrap();
        match (request.method.as_str(), segments.as_slice()) {
            ("GET", ["users", "me", "calendarList"]) => list_calendars(&state, request),
            ("GET", ["calendars", calendar, "events"]) => match state.calendars.get(*calendar) {
                Some(events) => list_events(events, request),
                None => error(404, "Not Found"),
            },
            ("POST", ["calendars", calendar, "events"]) => {
                state.writes.push(format!("POST {calendar}"));
                state.next_id += 1;
                let mut event: Value = serde_json::from_slice(&request.body).unwrap();
                event["id"] = json!(format!("evt{:03}", state.next_id));
                normalise_times(&mut event);
                state
                    .calendars
                    .entry((*calendar).to_owned())
                    .or_default()
                    .push(event.clone());
                Response::json(200, &event)
            }
            ("PATCH", ["calendars", calendar, "events", id]) => {
                state.writes.push(format!("PATCH {calendar} {id}"));
                let changes: Value = serde_json::from_slice(&request.body).unwrap();
                let Some(event) = find(&mut state, calendar, id) else {
                    return error(404, "Not Found");
                };
                for (key, value) in changes.as_object().unwrap() {
                    event[key] = value.clone();
                }
                normalise_times(event);
                Response::json(200, &json!({ "id": id }))
            }
            ("DELETE", ["calendars", calendar, "events", id]) => {
                state.writes.push(format!("DELETE {calendar} {id}"));
                let events = state.calendars.entry((*calendar).to_owned()).or_default();
                let before = events.len();
                events.retain(|event| event["id"] != *id);
                if events.len() == before {
                    return error(410, "Resource has been deleted");
                }
                Response {
                    status: 204,
                    body: Vec::new(),
                }
            }
            _ => error(404, "Not Found"),
        }
    }
}

fn find<'a>(state: &'a mut State, calendar: &str, id: &str) -> Option<&'a mut Value> {
    state
        .calendars
        .get_mut(calendar)?
        .iter_mut()
        .find(|event| event["id"] == id)
}

fn list_calendars(state: &State, request: &Request) -> Response {
    let items: Vec<Value> = state
        .calendars
        .keys()
        .map(|id| json!({ "id": id, "summary": format!("Calendar {id}"), "accessRole": "owner" }))
        .collect();
    page(items, request, 1)
}

fn list_events(events: &[Value], request: &Request) -> Response {
    assert_eq!(request.query("showDeleted"), Some("false"));
    let filter = request.query("privateExtendedProperty").map(|filter| {
        let (key, value) = filter.split_once('=').unwrap();
        (key.to_owned(), value.to_owned())
    });
    let items = events
        .iter()
        .filter(|event| match &filter {
            Some((key, value)) => event["extendedProperties"]["private"][key] == **value,
            None => true,
        })
        .cloned()
        .collect();
    page(items, request, PAGE_SIZE)
}

fn page(items: Vec<Value>, request: &Request, size: usize) -> Response {
    let offset: usize = request
        .query("pageToken")
        .map_or(0, |token| token.parse().unwrap());
    let end = (offset + size).min(items.len());
    let mut body = json!({ "items": items[offset..end] });
    if end < items.len() {
        body["nextPageToken"] = json!(end.to_string());
    }
    Response::json(200, &body)
}

/// Google echoes times in the calendar's zone; UTC here proves the tool
/// compares instants rather than strings.
fn normalise_times(event: &mut Value) {
    for key in ["start", "end"] {
        if let Some(date_time) = event[key]["dateTime"].as_str() {
            let utc = DateTime::parse_from_rfc3339(date_time)
                .unwrap()
                .with_timezone(&Utc);
            event[key]["dateTime"] = json!(utc.to_rfc3339_opts(SecondsFormat::Secs, true));
        }
    }
}

fn error(status: u16, message: &str) -> Response {
    Response::json(
        status,
        &json!({ "error": { "code": status, "message": message } }),
    )
}
