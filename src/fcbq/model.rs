use chrono::NaiveDateTime;
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// A scheduled or played match, as returned by `Match/getByTeamAllSeason`.
///
/// The backend types every scalar as a string and uses `null` or `""` for
/// absent values; both become `None` here. Only the fields this tool needs are
/// modelled, so new backend fields never break decoding.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchRecord {
    #[serde(deserialize_with = "id")]
    pub id_match: String,

    #[serde(default, deserialize_with = "text")]
    pub name_category: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub name_competition: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub name_group: Option<String>,
    #[serde(default, deserialize_with = "number")]
    pub num_match_day: Option<u32>,

    #[serde(default, deserialize_with = "text")]
    pub id_local_team: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub name_local_team: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub id_visitor_team: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub name_visitor_team: Option<String>,

    /// Tip-off in Catalan local time; `None` while the match has no date.
    #[serde(default, deserialize_with = "datetime")]
    pub match_day: Option<NaiveDateTime>,
    /// Set while a request to change the date, time or venue is pending.
    #[serde(default, deserialize_with = "text")]
    pub id_match_change_petition: Option<String>,

    #[serde(default, deserialize_with = "text")]
    pub name_field: Option<String>,
    #[serde(default, rename = "adressField", deserialize_with = "text")]
    pub address_field: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub postal_code_field: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub name_town: Option<String>,
}

/// Which side of a match a team plays on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Home,
    Away,
}

impl MatchRecord {
    /// The side the team plays on, or `None` when it does not play this match.
    pub fn side_of(&self, team_id: u64) -> Option<Side> {
        let team_id = team_id.to_string();
        if self.id_local_team.as_deref() == Some(&team_id) {
            Some(Side::Home)
        } else if self.id_visitor_team.as_deref() == Some(&team_id) {
            Some(Side::Away)
        } else {
            None
        }
    }

    /// Whether a schedule change petition is pending for this match.
    pub fn has_pending_change(&self) -> bool {
        self.id_match_change_petition
            .as_deref()
            .is_some_and(|id| id != "0")
    }
}

/// A club card from `FCBQWeb/fitxaClub`, reduced to the club's teams.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Club {
    #[serde(default, deserialize_with = "text")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "values")]
    pub categories: Vec<Category>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Category {
    #[serde(default, deserialize_with = "text")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "values")]
    pub teams: Vec<ClubTeam>,
}

/// A team's registration in a category for one season.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ClubTeam {
    /// The team id every other endpoint takes; it changes every season.
    #[serde(rename = "idSignedTeam", deserialize_with = "id")]
    pub id: String,
    #[serde(default, deserialize_with = "text")]
    pub name: Option<String>,
    #[serde(default, rename = "categoriesRegistredName", deserialize_with = "text")]
    pub category: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub season: Option<String>,
}

fn id<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    match Value::deserialize(de)? {
        Value::String(s) if !s.trim().is_empty() => Ok(s.trim().to_owned()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(D::Error::custom(format!("expected an id, got {other}"))),
    }
}

fn text<'de, D: Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(de)? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn number<'de, D: Deserializer<'de>>(de: D) -> Result<Option<u32>, D::Error> {
    Ok(text(de)?.and_then(|s| s.parse().ok()))
}

fn datetime<'de, D: Deserializer<'de>>(de: D) -> Result<Option<NaiveDateTime>, D::Error> {
    Ok(text(de)?.and_then(|s| NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S").ok()))
}

/// Accepts a collection served either as an array or as an object keyed by id.
fn values<'de, D, T>(de: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let items = match Value::deserialize(de)? {
        Value::Array(items) => items,
        Value::Object(map) => map.into_iter().map(|(_, item)| item).collect(),
        _ => Vec::new(),
    };
    items
        .into_iter()
        .map(|item| serde_json::from_value(item).map_err(D::Error::custom))
        .collect()
}
