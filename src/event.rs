//! What a match looks like as a calendar event. Event text is in Catalan, the
//! language of the federation's data and of the people sharing these calendars.

use chrono::{DateTime, Duration, TimeZone};
use chrono_tz::Tz;

use crate::config::Team;
use crate::fcbq::{MatchRecord, Side};

/// The zone FCBQ match times are expressed in, and events are scheduled in.
pub const TIME_ZONE: Tz = chrono_tz::Europe::Madrid;

const MATCH_PAGE_URL: &str = "https://www.basquetcatala.cat/partits/llistatpartits";

/// The calendar event a match should have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchEvent {
    pub match_id: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub start: DateTime<Tz>,
    pub end: DateTime<Tz>,
}

impl MatchEvent {
    /// The event for a match, or `None` while the match has no date.
    pub fn from_match(record: &MatchRecord, team: &Team, duration: Duration) -> Option<Self> {
        let start = TIME_ZONE
            .from_local_datetime(&record.match_day?)
            .earliest()?;
        Some(Self {
            match_id: record.id_match.clone(),
            summary: summary(record, team),
            location: location(record),
            description: description(record, record.side_of(team.fcbq_team_id)),
            start,
            end: start + duration,
        })
    }
}

/// `Home vs Away`, with the user's team shown by its configured label.
fn summary(record: &MatchRecord, team: &Team) -> String {
    let name = |side: Side, official: &Option<String>| {
        if record.side_of(team.fcbq_team_id) == Some(side) {
            team.label.clone()
        } else {
            official
                .clone()
                .unwrap_or_else(|| "Equip per determinar".to_owned())
        }
    };
    format!(
        "{} vs {}",
        name(Side::Home, &record.name_local_team),
        name(Side::Away, &record.name_visitor_team)
    )
}

fn location(record: &MatchRecord) -> String {
    let town = [&record.postal_code_field, &record.name_town]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    [
        record.name_field.clone(),
        record.address_field.clone(),
        Some(town),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(", ")
}

fn description(record: &MatchRecord, side: Option<Side>) -> String {
    let mut lines = Vec::new();
    if let Some(category) = &record.name_category {
        lines.push(format!("Categoria: {category}"));
    }
    let competition = [
        record.name_competition.clone(),
        record.name_group.clone(),
        record.num_match_day.map(|day| format!("Jornada {day}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !competition.is_empty() {
        lines.push(format!("Competició: {}", competition.join(" · ")));
    }
    match side {
        Some(Side::Home) => lines.push("Partit a casa".to_owned()),
        Some(Side::Away) => lines.push("Partit a fora".to_owned()),
        None => {}
    }
    if record.has_pending_change() {
        lines.push(String::new());
        lines.push(
            "⚠ Hi ha una petició de canvi d'horari pendent: la data, l'hora o el pavelló \
             encara poden canviar."
                .to_owned(),
        );
    }
    lines.push(String::new());
    lines.push(format!("{MATCH_PAGE_URL}/{}", record.id_match));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn team() -> Team {
        Team {
            label: "BAC Sènior M".into(),
            fcbq_team_id: 92430,
            calendar_id: "cal".into(),
        }
    }

    fn record() -> MatchRecord {
        serde_json::from_value(serde_json::json!({
            "idMatch": "23379",
            "nameCategory": "1A. TERRITORIAL SÈNIOR MASCULÍ",
            "nameCompetition": "1A TERRITORIAL SÈNIOR MASCULÍ -1A FASE",
            "nameGroup": "GRUP 01",
            "numMatchDay": "1",
            "idLocalTeam": "92430",
            "nameLocalTeam": "LA TAVERNA - BAC AGRAMUNT",
            "idVisitorTeam": "89987",
            "nameVisitorTeam": "COOPISTERIA PAPERS CB BELLPUIG B",
            "matchDay": "2026-09-26 18:00:00",
            "idMatchChangePetition": null,
            "nameField": "PAVELLO MUNICIPAL D'AGRAMUNT",
            "adressField": "PLAÇA FONDANDANA, S/N",
            "postalCodeField": "25310",
            "nameTown": "Agramunt"
        }))
        .unwrap()
    }

    fn event(record: &MatchRecord) -> MatchEvent {
        MatchEvent::from_match(record, &team(), Duration::minutes(120)).unwrap()
    }

    #[test]
    fn a_home_match_puts_the_team_label_first() {
        let event = event(&record());

        assert_eq!(
            event.summary,
            "BAC Sènior M vs COOPISTERIA PAPERS CB BELLPUIG B"
        );
        assert_eq!(
            event.location,
            "PAVELLO MUNICIPAL D'AGRAMUNT, PLAÇA FONDANDANA, S/N, 25310 Agramunt"
        );
        assert_eq!(
            event.description,
            "Categoria: 1A. TERRITORIAL SÈNIOR MASCULÍ\n\
             Competició: 1A TERRITORIAL SÈNIOR MASCULÍ -1A FASE · GRUP 01 · Jornada 1\n\
             Partit a casa\n\
             \n\
             https://www.basquetcatala.cat/partits/llistatpartits/23379"
        );
    }

    #[test]
    fn an_away_match_puts_the_team_label_second() {
        let mut record = record();
        std::mem::swap(&mut record.id_local_team, &mut record.id_visitor_team);
        std::mem::swap(&mut record.name_local_team, &mut record.name_visitor_team);

        let event = event(&record);

        assert_eq!(
            event.summary,
            "COOPISTERIA PAPERS CB BELLPUIG B vs BAC Sènior M"
        );
        assert!(event.description.contains("Partit a fora"));
    }

    #[test]
    fn times_are_madrid_local_time_across_daylight_saving() {
        let summer = event(&record());
        assert_eq!(summer.start.to_rfc3339(), "2026-09-26T18:00:00+02:00");
        assert_eq!(summer.end.to_rfc3339(), "2026-09-26T20:00:00+02:00");

        let mut winter = record();
        winter.match_day = NaiveDate::from_ymd_opt(2027, 1, 9)
            .unwrap()
            .and_hms_opt(18, 0, 0);
        assert_eq!(
            event(&winter).start.to_rfc3339(),
            "2027-01-09T18:00:00+01:00"
        );
    }

    #[test]
    fn a_pending_change_petition_is_noted() {
        let mut record = record();
        record.id_match_change_petition = Some("812".into());

        assert!(
            event(&record)
                .description
                .contains("petició de canvi d'horari pendent")
        );
    }

    #[test]
    fn a_match_without_a_date_has_no_event() {
        let mut record = record();
        record.match_day = None;

        assert_eq!(
            MatchEvent::from_match(&record, &team(), Duration::minutes(90)),
            None
        );
    }

    #[test]
    fn missing_venue_parts_are_skipped() {
        let mut record = record();
        record.address_field = None;
        record.postal_code_field = None;

        assert_eq!(
            event(&record).location,
            "PAVELLO MUNICIPAL D'AGRAMUNT, Agramunt"
        );
    }
}
