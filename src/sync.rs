//! Idempotent sync of a team's FCBQ schedule into its Google calendar.
//!
//! The tool owns exactly the events carrying its private markers: the FCBQ team
//! id and match id. Events without both markers for the team being synced are
//! never read into a plan, so they are never updated or deleted.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, FixedOffset};

use crate::config::{Config, Team};
use crate::event::{MatchEvent, TIME_ZONE};
use crate::fcbq::{self, MatchRecord};
use crate::google::{CalendarClient, Event, EventTime, ExtendedProperties};

/// Private extended property holding the FCBQ team id of a managed event.
pub const TEAM_ID_PROPERTY: &str = "bacCalendarTeamId";
/// Private extended property holding the FCBQ match id of a managed event.
pub const MATCH_ID_PROPERTY: &str = "bacCalendarMatchId";

/// The writes that bring a team's calendar in line with its schedule.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub creates: Vec<MatchEvent>,
    pub updates: Vec<Update>,
    pub deletes: Vec<Delete>,
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Update {
    pub event_id: String,
    pub event: MatchEvent,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Title,
    Time,
    Venue,
    Details,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Change::Title => "title",
            Change::Time => "time",
            Change::Venue => "venue",
            Change::Details => "details",
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Delete {
    pub event_id: String,
    pub summary: String,
    pub start: Option<DateTime<FixedOffset>>,
    pub reason: DeleteReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteReason {
    /// The match left the schedule or lost its date.
    NotScheduled,
    /// Another managed event already stands for the same match.
    Duplicate,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.creates.is_empty() && self.updates.is_empty() && self.deletes.is_empty()
    }
}

/// Plans the writes for one team.
///
/// `existing` is what the calendar returned for the team's marker; any event
/// in it without the team's markers is ignored. An empty schedule never deletes
/// anything, since it more likely means a stale team id or a backend hiccup
/// than a season with no matches.
pub fn plan(team: &Team, matches: &[MatchRecord], existing: &[Event], duration: Duration) -> Plan {
    let mut desired = BTreeMap::new();
    for record in matches {
        if let Some(event) = MatchEvent::from_match(record, team, duration) {
            desired.entry(event.match_id.clone()).or_insert(event);
        }
    }

    let team_id = team.fcbq_team_id.to_string();
    let mut managed: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for event in existing {
        if event.id.is_none() || event.private_property(TEAM_ID_PROPERTY) != Some(&team_id) {
            continue;
        }
        if let Some(match_id) = event.private_property(MATCH_ID_PROPERTY) {
            managed.entry(match_id).or_default().push(event);
        }
    }

    let mut plan = Plan::default();
    for (match_id, events) in &mut managed {
        events.sort_by(|a, b| a.id.cmp(&b.id));
        let (kept, duplicates) = events
            .split_first()
            .expect("grouped events are never empty");
        plan.deletes.extend(
            duplicates
                .iter()
                .map(|event| delete(event, DeleteReason::Duplicate)),
        );
        match desired.get(*match_id) {
            Some(wanted) => {
                let changes = changes(kept, wanted);
                if !changes.is_empty() {
                    plan.updates.push(Update {
                        event_id: kept.id.clone().expect("managed events have ids"),
                        event: wanted.clone(),
                        changes,
                    });
                }
            }
            None if matches.is_empty() => {}
            None => plan.deletes.push(delete(kept, DeleteReason::NotScheduled)),
        }
    }
    if matches.is_empty() && !managed.is_empty() {
        plan.warnings.push(format!(
            "FCBQ returned no matches for team {}, so its {} events were kept; \
             if the season changed, look up the team's new id",
            team.fcbq_team_id,
            managed.len()
        ));
    }

    let managed_ids: BTreeSet<_> = managed.keys().copied().collect();
    plan.creates = desired
        .into_values()
        .filter(|event| !managed_ids.contains(event.match_id.as_str()))
        .collect();
    plan.creates.sort_by_key(|event| event.start);
    plan.updates.sort_by_key(|update| update.event.start);
    plan
}

fn changes(existing: &Event, wanted: &MatchEvent) -> Vec<Change> {
    let text = |value: &Option<String>| value.as_deref().unwrap_or_default().to_owned();
    let instant = |time: &Option<EventTime>| time.as_ref().and_then(EventTime::instant);

    let mut changes = Vec::new();
    if text(&existing.summary) != wanted.summary {
        changes.push(Change::Title);
    }
    if instant(&existing.start) != Some(wanted.start.fixed_offset())
        || instant(&existing.end) != Some(wanted.end.fixed_offset())
    {
        changes.push(Change::Time);
    }
    if text(&existing.location) != wanted.location {
        changes.push(Change::Venue);
    }
    if text(&existing.description) != wanted.description {
        changes.push(Change::Details);
    }
    changes
}

fn delete(event: &Event, reason: DeleteReason) -> Delete {
    Delete {
        event_id: event.id.clone().expect("managed events have ids"),
        summary: event.summary.clone().unwrap_or_default(),
        start: event.start.as_ref().and_then(EventTime::instant),
        reason,
    }
}

/// The event body for a match: its content plus, on creation, the markers.
fn event_body(event: &MatchEvent, markers: Option<&Team>) -> Event {
    let time = |instant: DateTime<chrono_tz::Tz>| EventTime {
        date_time: Some(instant.to_rfc3339()),
        date: None,
        time_zone: Some(TIME_ZONE.name().to_owned()),
    };
    Event {
        id: None,
        summary: Some(event.summary.clone()),
        location: Some(event.location.clone()),
        description: Some(event.description.clone()),
        start: Some(time(event.start)),
        end: Some(time(event.end)),
        extended_properties: markers.map(|team| ExtendedProperties {
            private: BTreeMap::from([
                (TEAM_ID_PROPERTY.to_owned(), team.fcbq_team_id.to_string()),
                (MATCH_ID_PROPERTY.to_owned(), event.match_id.clone()),
            ]),
        }),
    }
}

/// Applies a plan to the team's calendar.
pub fn apply(plan: &Plan, team: &Team, calendar: &CalendarClient) -> Result<()> {
    for event in &plan.creates {
        calendar
            .insert_event(&team.calendar_id, &event_body(event, Some(team)))
            .with_context(|| format!("creating \"{}\"", event.summary))?;
    }
    for update in &plan.updates {
        calendar
            .patch_event(
                &team.calendar_id,
                &update.event_id,
                &event_body(&update.event, None),
            )
            .with_context(|| format!("updating \"{}\"", update.event.summary))?;
    }
    for delete in &plan.deletes {
        calendar
            .delete_event(&team.calendar_id, &delete.event_id)
            .with_context(|| format!("deleting \"{}\"", delete.summary))?;
    }
    Ok(())
}

/// Syncs every configured team, reporting progress to `out`.
///
/// A failing team does not stop the others; the run fails at the end if any did.
pub fn run(
    config: &Config,
    fcbq: &fcbq::Client,
    calendar: &CalendarClient,
    dry_run: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let mut failed = Vec::new();
    for team in &config.teams {
        writeln!(
            out,
            "{} (FCBQ team {}) -> {}",
            team.label, team.fcbq_team_id, team.calendar_id
        )?;
        if let Err(err) = sync_team(config, team, fcbq, calendar, dry_run, out) {
            writeln!(out, "  error: {err:#}")?;
            failed.push(team.label.as_str());
        }
    }
    if !failed.is_empty() {
        bail!(
            "{} of {} teams failed: {}",
            failed.len(),
            config.teams.len(),
            failed.join(", ")
        );
    }
    Ok(())
}

fn sync_team(
    config: &Config,
    team: &Team,
    fcbq: &fcbq::Client,
    calendar: &CalendarClient,
    dry_run: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let matches = fcbq
        .team_season(team.fcbq_team_id)
        .context("fetching the FCBQ schedule")?;
    let existing = calendar
        .events_with_private_property(
            &team.calendar_id,
            TEAM_ID_PROPERTY,
            &team.fcbq_team_id.to_string(),
        )
        .context("reading the calendar")?;
    let plan = plan(team, &matches, &existing, config.event_duration());

    report(&plan, out)?;
    if plan.is_empty() {
        writeln!(out, "  up to date ({} matches)", matches.len())?;
        return Ok(());
    }
    if dry_run {
        writeln!(
            out,
            "  dry run: {}",
            totals(&plan, "to create", "to update", "to delete")
        )?;
        return Ok(());
    }
    apply(&plan, team, calendar)?;
    writeln!(out, "  {}", totals(&plan, "created", "updated", "deleted"))?;
    Ok(())
}

fn report(plan: &Plan, out: &mut dyn Write) -> Result<()> {
    let when = |start: Option<DateTime<FixedOffset>>| {
        start.map_or_else(
            || "????-??-?? ??:??".to_owned(),
            |start| {
                start
                    .with_timezone(&TIME_ZONE)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            },
        )
    };
    for warning in &plan.warnings {
        writeln!(out, "  warning: {warning}")?;
    }
    for event in &plan.creates {
        writeln!(
            out,
            "  create  {}  {}",
            when(Some(event.start.fixed_offset())),
            event.summary
        )?;
    }
    for update in &plan.updates {
        let changes = update
            .changes
            .iter()
            .map(Change::to_string)
            .collect::<Vec<_>>();
        writeln!(
            out,
            "  update  {}  {} ({})",
            when(Some(update.event.start.fixed_offset())),
            update.event.summary,
            changes.join(", ")
        )?;
    }
    for delete in &plan.deletes {
        let reason = match delete.reason {
            DeleteReason::NotScheduled => "no longer scheduled",
            DeleteReason::Duplicate => "duplicate",
        };
        writeln!(
            out,
            "  delete  {}  {} ({reason})",
            when(delete.start),
            delete.summary
        )?;
    }
    Ok(())
}

fn totals(plan: &Plan, created: &str, updated: &str, deleted: &str) -> String {
    format!(
        "{} {created}, {} {updated}, {} {deleted}",
        plan.creates.len(),
        plan.updates.len(),
        plan.deletes.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEASON: &[u8] = include_bytes!("../tests/fixtures/fcbq-season-92430.b64");

    fn team() -> Team {
        Team {
            label: "BAC Sènior M".into(),
            fcbq_team_id: 92430,
            calendar_id: "cal".into(),
        }
    }

    fn season() -> Vec<MatchRecord> {
        fcbq::decode_envelope(SEASON).unwrap()
    }

    fn duration() -> Duration {
        Duration::minutes(120)
    }

    /// The calendar as it stands after applying `plan`'s creates, with ids.
    fn created(plan: &Plan) -> Vec<Event> {
        plan.creates
            .iter()
            .enumerate()
            .map(|(n, event)| Event {
                id: Some(format!("evt{n:03}")),
                ..event_body(event, Some(&team()))
            })
            .collect()
    }

    fn in_sync() -> Vec<Event> {
        created(&plan(&team(), &season(), &[], duration()))
    }

    fn find<'a>(events: &'a mut [Event], match_id: &str) -> &'a mut Event {
        events
            .iter_mut()
            .find(|event| event.private_property(MATCH_ID_PROPERTY) == Some(match_id))
            .unwrap()
    }

    #[test]
    fn an_empty_calendar_gets_every_match() {
        let plan = plan(&team(), &season(), &[], duration());

        assert_eq!(plan.creates.len(), 24);
        assert!(plan.updates.is_empty() && plan.deletes.is_empty() && plan.warnings.is_empty());
        assert_eq!(plan.creates[0].match_id, "23379");
        assert_eq!(
            plan.creates[0].summary,
            "BAC Sènior M vs COOPISTERIA PAPERS CB BELLPUIG B"
        );
        assert_eq!(
            plan.creates[1].summary,
            "CB CERVERA MOBEL LINEA VERD vs BAC Sènior M"
        );
    }

    #[test]
    fn a_calendar_in_sync_needs_nothing() {
        assert_eq!(
            plan(&team(), &season(), &in_sync(), duration()),
            Plan::default()
        );
    }

    #[test]
    fn the_same_instant_in_another_offset_is_not_a_change() {
        let mut events = in_sync();
        let event = find(&mut events, "23379");
        event.start.as_mut().unwrap().date_time = Some("2026-09-26T16:00:00Z".into());
        event.end.as_mut().unwrap().date_time = Some("2026-09-26T18:00:00Z".into());

        assert!(plan(&team(), &season(), &events, duration()).is_empty());
    }

    #[test]
    fn a_rescheduled_match_is_updated() {
        let mut matches = season();
        matches[0].match_day = matches[0].match_day.map(|day| day + Duration::hours(1));

        let plan = plan(&team(), &matches, &in_sync(), duration());

        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].event_id, "evt000");
        assert_eq!(plan.updates[0].changes, [Change::Time]);
        assert_eq!(
            plan.updates[0].event.start.to_rfc3339(),
            "2026-09-26T19:00:00+02:00"
        );
        assert!(plan.creates.is_empty() && plan.deletes.is_empty());
    }

    #[test]
    fn venue_opponent_and_petition_changes_are_updated() {
        let mut matches = season();
        matches[0].name_field = Some("PAVELLO NOU".into());
        matches[1].name_local_team = Some("CB CERVERA".into());
        matches[2].id_match_change_petition = Some("77".into());

        let plan = plan(&team(), &matches, &in_sync(), duration());

        let changes: Vec<_> = plan
            .updates
            .iter()
            .map(|update| update.changes.clone())
            .collect();
        assert_eq!(
            changes,
            [
                vec![Change::Venue],
                vec![Change::Title],
                vec![Change::Details]
            ]
        );
    }

    #[test]
    fn a_hand_edited_managed_event_is_restored() {
        let mut events = in_sync();
        find(&mut events, "23395").summary = Some("Partit".into());
        find(&mut events, "23395").start.as_mut().unwrap().date_time = None;

        let plan = plan(&team(), &season(), &events, duration());

        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].changes, [Change::Title, Change::Time]);
    }

    #[test]
    fn a_match_that_left_the_schedule_is_deleted() {
        let mut matches = season();
        let removed = matches.remove(3);

        let plan = plan(&team(), &matches, &in_sync(), duration());

        assert_eq!(plan.deletes.len(), 1);
        assert_eq!(plan.deletes[0].reason, DeleteReason::NotScheduled);
        assert_eq!(plan.deletes[0].summary, "CN TÀRREGA vs BAC Sènior M");
        assert_eq!(removed.id_match, "23405");
        assert!(plan.creates.is_empty() && plan.updates.is_empty());
    }

    #[test]
    fn a_match_that_lost_its_date_is_deleted() {
        let mut matches = season();
        matches[5].match_day = None;

        let plan = plan(&team(), &matches, &in_sync(), duration());

        assert_eq!(plan.deletes.len(), 1);
        assert_eq!(plan.deletes[0].reason, DeleteReason::NotScheduled);
    }

    #[test]
    fn duplicate_managed_events_are_collapsed() {
        let mut events = in_sync();
        let mut duplicate = events[0].clone();
        duplicate.id = Some("zzz".into());
        events.push(duplicate);

        let plan = plan(&team(), &season(), &events, duration());

        assert_eq!(plan.deletes.len(), 1);
        assert_eq!(plan.deletes[0].event_id, "zzz");
        assert_eq!(plan.deletes[0].reason, DeleteReason::Duplicate);
        assert!(plan.creates.is_empty() && plan.updates.is_empty());
    }

    #[test]
    fn events_without_the_team_marker_are_never_touched() {
        let mut stray = in_sync();
        let mut events = vec![
            Event {
                id: Some("hand".into()),
                summary: Some("Entrenament".into()),
                ..Event::default()
            },
            Event {
                id: Some("other-team".into()),
                extended_properties: Some(ExtendedProperties {
                    private: BTreeMap::from([
                        (TEAM_ID_PROPERTY.into(), "1".into()),
                        (MATCH_ID_PROPERTY.into(), "999".into()),
                    ]),
                }),
                ..Event::default()
            },
        ];
        let mut no_match_marker = stray.remove(0);
        no_match_marker
            .extended_properties
            .as_mut()
            .unwrap()
            .private
            .remove(MATCH_ID_PROPERTY);
        no_match_marker.id = Some("half-marked".into());
        events.push(no_match_marker);
        events.extend(stray);

        let plan = plan(&team(), &season(), &events, duration());

        assert_eq!(
            plan.creates.len(),
            1,
            "the unmarked copy does not count as the match"
        );
        assert!(plan.updates.is_empty() && plan.deletes.is_empty());
    }

    #[test]
    fn an_empty_schedule_deletes_nothing() {
        let plan = plan(&team(), &[], &in_sync(), duration());

        assert!(plan.is_empty());
        assert_eq!(plan.warnings.len(), 1);
        assert!(
            plan.warnings[0].contains("24 events were kept"),
            "{}",
            plan.warnings[0]
        );
    }

    #[test]
    fn created_events_carry_both_markers_and_the_madrid_zone() {
        let body = event_body(
            &plan(&team(), &season(), &[], duration()).creates[0],
            Some(&team()),
        );

        assert_eq!(body.private_property(TEAM_ID_PROPERTY), Some("92430"));
        assert_eq!(body.private_property(MATCH_ID_PROPERTY), Some("23379"));
        let start = body.start.unwrap();
        assert_eq!(
            start.date_time.as_deref(),
            Some("2026-09-26T18:00:00+02:00")
        );
        assert_eq!(start.time_zone.as_deref(), Some("Europe/Madrid"));
    }

    #[test]
    fn updates_leave_the_markers_alone() {
        let body = event_body(&plan(&team(), &season(), &[], duration()).creates[0], None);
        assert_eq!(body.extended_properties, None);
    }
}
