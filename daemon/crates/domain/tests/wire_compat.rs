//! Decode-compatibility with the original F#/Fable codec.
//!
//! `fixtures/fsharp_wire_fixtures.jsonl` was captured verbatim from the F#
//! parity program before the migration. Every message the old codec ever
//! wrote (including what sits in users' databases) must decode, and
//! re-encoding must round-trip to an equal value.

use temporal_domain::wire::*;
use temporal_domain::{IpcRequest, IpcResponse, WorkspaceState};

const FIXTURES: &str = include_str!("fixtures/fsharp_wire_fixtures.jsonl");

fn lines() -> Vec<&'static str> {
    FIXTURES.lines().filter(|l| !l.trim().is_empty()).collect()
}

/// Fixture layout (from shared/Temporal.Parity/Fixtures.fs, now deleted):
/// 0-1 raw JSON edge cases, 2-3 workspaces, 4-7 requests, 8-14 responses.
#[test]
fn old_workspaces_decode_and_roundtrip() {
    for line in &lines()[2..4] {
        let decoded: WorkspaceState =
            workspace_from_wire(line).unwrap_or_else(|e| panic!("decode failed: {e}\n{line}"));
        let reencoded = workspace_to_wire(&decoded);
        let again = workspace_from_wire(&reencoded).expect("re-decode");
        assert_eq!(decoded, again, "round-trip mismatch for: {line}");
    }
}

#[test]
fn old_requests_decode_and_roundtrip() {
    for line in &lines()[4..8] {
        let decoded: IpcRequest =
            request_from_wire(line).unwrap_or_else(|e| panic!("decode failed: {e}\n{line}"));
        let again = request_from_wire(&request_to_wire(&decoded)).expect("re-decode");
        assert_eq!(decoded, again, "round-trip mismatch for: {line}");
    }
}

#[test]
fn old_responses_decode_and_roundtrip() {
    for line in &lines()[8..15] {
        let decoded: IpcResponse =
            response_from_wire(line).unwrap_or_else(|e| panic!("decode failed: {e}\n{line}"));
        let again = response_from_wire(&response_to_wire(&decoded)).expect("re-decode");
        assert_eq!(decoded, again, "round-trip mismatch for: {line}");
    }
}

#[test]
fn rich_fixture_content_survives() {
    // Spot-check that unicode titles, escapes and numbers came through the
    // old format intact, not just that decoding didn't error.
    let ws = workspace_from_wire(lines()[2]).expect("rich workspace");
    assert_eq!(ws.workspace_id, "ws-rich");
    assert_eq!(ws.captured_at_unix_ms, 1752300000000);
    assert!(ws.tags.contains(&"日本語".to_string()));
    let chrome = &ws.nodes[0];
    assert_eq!(chrome.window_title, "Fable · docs");
    assert_eq!(chrome.geometry.height, 875.5);
    match &chrome.payload {
        temporal_domain::NodePayload::Browser { tabs, active_tab_index } => {
            assert_eq!(*active_tab_index, 1);
            assert_eq!(tabs[1].title, "GitHub — \"Fable\"");
        }
        other => panic!("unexpected payload: {other:?}"),
    }
    let terminal = &ws.nodes[1];
    match &terminal.payload {
        temporal_domain::NodePayload::Terminal { tabs } => {
            assert_eq!(tabs[0].cwd, "/Users/dev/temporal");
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn wire_tags_match_the_old_codec_exactly() {
    // The new encoder must speak the same dialect for every variant tag.
    assert_eq!(request_to_wire(&IpcRequest::Ping), r#"{"type":"ping"}"#);
    assert_eq!(response_to_wire(&IpcResponse::Pong), r#"{"type":"pong"}"#);
    assert_eq!(
        response_to_wire(&IpcResponse::FreezeStarted { workspace_id: "w".into() }),
        r#"{"type":"freeze-started","workspaceId":"w"}"#
    );
    assert_eq!(
        response_to_wire(&IpcResponse::Error { code: "E".into(), message: "m".into() }),
        r#"{"type":"error","code":"E","message":"m"}"#
    );
    assert_eq!(
        request_to_wire(&IpcRequest::Query { text: "t".into(), limit: 5 }),
        r#"{"type":"query","text":"t","limit":5}"#
    );
}
