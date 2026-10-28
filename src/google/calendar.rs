use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use percent_encoding::utf8_percent_encode;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://www.googleapis.com/calendar/v3";

const EVENT_FIELDS: &str = "id,summary,location,description,start,end,extendedProperties";

/// A Calendar API event, reduced to the fields this tool reads and writes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<EventTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<EventTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extended_properties: Option<ExtendedProperties>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_time: Option<String>,
    /// Set instead of `date_time` on all-day events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

impl EventTime {
    /// The instant of a timed event; `None` for all-day or unreadable times.
    pub fn instant(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(self.date_time.as_deref()?).ok()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtendedProperties {
    /// Properties visible only to this calendar's copy of the event.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub private: BTreeMap<String, String>,
}

impl Event {
    pub fn private_property(&self, key: &str) -> Option<&str> {
        self.extended_properties
            .as_ref()?
            .private
            .get(key)
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarListEntry {
    pub id: String,
    #[serde(default)]
    pub summary: Option<String>,
    /// The user's own name for a calendar, which wins over `summary`.
    #[serde(default)]
    pub summary_override: Option<String>,
    #[serde(default)]
    pub access_role: Option<String>,
    #[serde(default)]
    pub primary: bool,
}

impl CalendarListEntry {
    pub fn name(&self) -> &str {
        self.summary_override
            .as_deref()
            .or(self.summary.as_deref())
            .unwrap_or("")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    next_page_token: Option<String>,
}

/// Calendar API v3 client authorised by an OAuth access token.
pub struct CalendarClient {
    base_url: String,
    access_token: String,
    http: ureq::Agent,
}

impl CalendarClient {
    pub fn new(base_url: impl Into<String>, access_token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            access_token: access_token.into(),
            http: super::agent(),
        }
    }

    /// Every calendar in the user's calendar list.
    pub fn calendars(&self) -> Result<Vec<CalendarListEntry>> {
        let url = format!("{}/users/me/calendarList", self.base_url);
        self.list_all(
            &url,
            "listing calendars",
            &[
                ("maxResults", "250"),
                (
                    "fields",
                    "nextPageToken,items(id,summary,summaryOverride,accessRole,primary)",
                ),
            ],
        )
    }

    /// The events of a calendar carrying the private property `key=value`.
    pub fn events_with_private_property(
        &self,
        calendar_id: &str,
        key: &str,
        value: &str,
    ) -> Result<Vec<Event>> {
        let filter = format!("{key}={value}");
        let fields = format!("nextPageToken,items({EVENT_FIELDS})");
        self.list_all(
            &self.events_url(calendar_id),
            "listing events",
            &[
                ("privateExtendedProperty", filter.as_str()),
                ("showDeleted", "false"),
                ("maxResults", "2500"),
                ("fields", fields.as_str()),
            ],
        )
    }

    pub fn insert_event(&self, calendar_id: &str, event: &Event) -> Result<Event> {
        let response = self
            .authorised(self.http.post(self.events_url(calendar_id)))
            .query("sendUpdates", "none")
            .query("fields", EVENT_FIELDS)
            .send_json(event)?;
        Ok(super::check(response, "creating an event")?.read_json()?)
    }

    /// Overwrites the fields set in `changes`, leaving the rest of the event as is.
    pub fn patch_event(&self, calendar_id: &str, event_id: &str, changes: &Event) -> Result<()> {
        let response = self
            .authorised(self.http.patch(self.event_url(calendar_id, event_id)))
            .query("sendUpdates", "none")
            .query("fields", "id")
            .send_json(changes)?;
        super::check(response, "updating an event")?;
        Ok(())
    }

    /// Deletes an event; one that is already gone counts as deleted.
    pub fn delete_event(&self, calendar_id: &str, event_id: &str) -> Result<()> {
        let response = self
            .authorised(self.http.delete(self.event_url(calendar_id, event_id)))
            .query("sendUpdates", "none")
            .call()?;
        if matches!(response.status().as_u16(), 404 | 410) {
            return Ok(());
        }
        super::check(response, "deleting an event")?;
        Ok(())
    }

    fn list_all<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        what: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<T>> {
        let mut items = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut request = self
                .authorised(self.http.get(url))
                .query_pairs(query.iter().copied());
            if let Some(token) = &page_token {
                request = request.query("pageToken", token);
            }
            let page: Page<T> = super::check(request.call()?, what)?
                .read_json()
                .with_context(|| format!("{what}: unexpected response"))?;
            items.extend(page.items);
            match page.next_page_token {
                Some(token) => page_token = Some(token),
                None => return Ok(items),
            }
        }
    }

    fn authorised<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        request.header("Authorization", format!("Bearer {}", self.access_token))
    }

    fn events_url(&self, calendar_id: &str) -> String {
        format!(
            "{}/calendars/{}/events",
            self.base_url,
            utf8_percent_encode(calendar_id, super::UNRESERVED)
        )
    }

    fn event_url(&self, calendar_id: &str, event_id: &str) -> String {
        format!(
            "{}/{}",
            self.events_url(calendar_id),
            utf8_percent_encode(event_id, super::UNRESERVED)
        )
    }
}

impl fmt::Debug for CalendarClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CalendarClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}
