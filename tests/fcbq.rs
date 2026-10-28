//! FCBQ decoding against captured responses, and the client over local HTTP.

mod support;

use bac_calendar::fcbq::{self, Club, Error, MatchRecord, Side, decode_envelope};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;
use support::{FCBQ_ACCESS_DENIED, FCBQ_SEASON, Response, StubServer};

/// Wraps a payload the way the backend does, base64 included.
fn envelope(message_data: serde_json::Value) -> String {
    STANDARD.encode(
        json!({ "result": "OK", "errorCode": 0, "message": "", "messageData": message_data })
            .to_string(),
    )
}

#[test]
fn decodes_a_season_calendar() {
    let matches: Vec<MatchRecord> = decode_envelope(FCBQ_SEASON).unwrap();

    assert_eq!(matches.len(), 24);
    let first = &matches[0];
    assert_eq!(first.id_match, "23379");
    assert_eq!(first.num_match_day, Some(1));
    assert_eq!(first.match_day.unwrap().to_string(), "2026-09-26 18:00:00");
    assert_eq!(
        first.name_local_team.as_deref(),
        Some("LA TAVERNA - BAC AGRAMUNT")
    );
    assert_eq!(
        first.name_field.as_deref(),
        Some("PAVELLO MUNICIPAL D'AGRAMUNT")
    );
    assert_eq!(
        first.address_field.as_deref(),
        Some("PLAÇA FONDANDANA, S/N")
    );
    assert_eq!(first.side_of(92430), Some(Side::Home));
    assert_eq!(first.side_of(1), None);
    assert!(!first.has_pending_change());
    assert!(matches.iter().all(|m| m.match_day.is_some()));
    assert!(matches.iter().all(|m| m.side_of(92430).is_some()));
}

#[test]
fn trims_padded_text() {
    let matches: Vec<MatchRecord> = decode_envelope(FCBQ_SEASON).unwrap();
    let balaguer = matches.iter().find(|m| m.id_match == "23441").unwrap();
    assert_eq!(balaguer.name_field.as_deref(), Some("PAVELLÓ 1R D'OCTUBRE"));
}

#[test]
fn a_plain_json_error_is_a_typed_error() {
    match decode_envelope::<Vec<MatchRecord>>(FCBQ_ACCESS_DENIED) {
        Err(Error::Api(err)) => {
            assert_eq!(err.error_code, "ERRORCODEx0009");
            assert!(err.is_access_denied());
        }
        other => panic!("expected an API error, got {other:?}"),
    }
}

#[test]
fn a_base64_error_is_a_typed_error() {
    let body = STANDARD.encode(
        r#"{"result":"ERROR","errorCode":11,"message":"The requested object does not exist","messageData":[]}"#,
    );
    match decode_envelope::<Vec<MatchRecord>>(body.as_bytes()) {
        Err(Error::Api(err)) => {
            assert_eq!(err.error_code, "11");
            assert!(!err.is_access_denied());
        }
        other => panic!("expected an API error, got {other:?}"),
    }
}

#[test]
fn tolerates_unknown_fields_and_empty_strings() {
    let body = envelope(json!([{
        "idMatch": 1,
        "matchDay": "",
        "nameField": "  ",
        "idMatchChangePetition": "0",
        "aBrandNewField": { "nested": [1, 2, 3] }
    }]));

    let matches: Vec<MatchRecord> = decode_envelope(body.as_bytes()).unwrap();

    assert_eq!(matches[0].id_match, "1");
    assert_eq!(matches[0].match_day, None);
    assert_eq!(matches[0].name_field, None);
    assert!(!matches[0].has_pending_change());
}

#[test]
fn rejects_a_body_that_is_neither_base64_nor_json() {
    let decoded = decode_envelope::<Vec<MatchRecord>>(b"<html>maintenance</html>");
    assert!(
        matches!(decoded, Err(Error::Undecodable)),
        "got {decoded:?}"
    );
}

#[test]
fn reports_an_unexpected_payload_shape() {
    let decoded = decode_envelope::<Vec<MatchRecord>>(envelope(json!({"no": "array"})).as_bytes());
    assert!(
        matches!(decoded, Err(Error::Malformed(_))),
        "got {decoded:?}"
    );
}

#[test]
fn decodes_a_club_with_teams_keyed_by_id() {
    let body = envelope(json!({
        "id": "393",
        "name": "BASQUET AGRAMUNT CLUB",
        "categories": {
            "43": { "name": "SÈNIOR MASCULÍ", "teams": {
                "92430": {
                    "idSignedTeam": "92430",
                    "name": "LA TAVERNA - BAC AGRAMUNT",
                    "categoriesRegistredName": "1A. TERRITORIAL SÈNIOR MASCULÍ",
                    "season": "2026"
                }
            } },
            "51": { "name": "MINI FEMENÍ", "teams": [
                { "idSignedTeam": 92432, "name": "BAC AGRAMUNT", "categoriesRegistredName": null }
            ] }
        }
    }));

    let club: Club = decode_envelope(body.as_bytes()).unwrap();

    assert_eq!(club.name.as_deref(), Some("BASQUET AGRAMUNT CLUB"));
    let ids: Vec<_> = club
        .categories
        .iter()
        .flat_map(|c| &c.teams)
        .map(|t| &t.id)
        .collect();
    assert_eq!(ids, ["92430", "92432"]);
    assert_eq!(
        club.categories[0].teams[0].category.as_deref(),
        Some("1A. TERRITORIAL SÈNIOR MASCULÍ")
    );
    assert_eq!(club.categories[1].teams[0].category, None);
}

#[test]
fn the_client_fetches_through_the_key_path() {
    let server = StubServer::start(|request| match request.path.as_str() {
        "/fcbq/1/secret-key/Match/getByTeamAllSeason/92430" => Response::ok(FCBQ_SEASON),
        "/fcbq/1/secret-key/FCBQWeb/fitxaClub/393" => {
            Response::ok(envelope(json!({ "name": "BAC", "categories": {} })))
        }
        _ => Response::ok(FCBQ_ACCESS_DENIED),
    });
    let client = fcbq::Client::new(format!("{}/fcbq/1/", server.base_url), "secret-key");

    assert_eq!(client.team_season(92430).unwrap().len(), 24);
    assert_eq!(client.club(393).unwrap().name.as_deref(), Some("BAC"));
    assert!(matches!(client.team_season(1), Err(Error::Api(err)) if err.is_access_denied()));
}

#[test]
fn http_200_with_an_error_envelope_is_never_a_success() {
    let server = StubServer::start(|_| Response::ok(FCBQ_ACCESS_DENIED));
    let client = fcbq::Client::new(&server.base_url, "k");

    let err = client.team_season(92430).unwrap_err();

    assert_eq!(
        err.to_string(),
        "FCBQ denied access (ERRORCODEx0009): the API key is wrong or has been rotated"
    );
}

#[test]
fn a_non_envelope_http_failure_reports_the_status() {
    let server = StubServer::start(|_| Response {
        status: 502,
        body: b"Bad Gateway".to_vec(),
    });
    let client = fcbq::Client::new(&server.base_url, "k");

    assert!(matches!(client.team_season(1), Err(Error::Status(502))));
}

#[test]
fn transport_errors_never_reveal_the_key() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client = fcbq::Client::new(format!("http://127.0.0.1:{port}"), "very-secret-key");

    let err = client.team_season(92430).unwrap_err();

    assert!(matches!(err, Error::Transport(_)), "got {err:?}");
    assert!(!format!("{err} {err:?} {client:?}").contains("very-secret-key"));
}
