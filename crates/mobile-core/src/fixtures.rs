#[cfg(test)]
mod tests {
    use crate::{
        BridgeCommand, BridgeResult, BridgeSession, MobileEvent, VocabSnapshot, has_term,
        is_newer_sequence,
    };

    #[test]
    fn bridge_fixtures_parse() {
        let session: BridgeSession = serde_json::from_str(include_str!(
            "../../../mobile/shared/fixtures/bridge_session.json"
        ))
        .expect("bridge_session fixture should parse");
        let command: BridgeCommand = serde_json::from_str(include_str!(
            "../../../mobile/shared/fixtures/bridge_command.json"
        ))
        .expect("bridge_command fixture should parse");
        let result: BridgeResult = serde_json::from_str(include_str!(
            "../../../mobile/shared/fixtures/bridge_result.json"
        ))
        .expect("bridge_result fixture should parse");

        assert!(is_newer_sequence(session.result_seq, result.result_seq));
        assert!(is_newer_sequence(session.command_seq, command.command_seq));
    }

    #[test]
    fn event_fixture_parses() {
        let event: MobileEvent = serde_json::from_str(include_str!(
            "../../../mobile/shared/fixtures/mobile_event.json"
        ))
        .expect("mobile_event fixture should parse");
        assert_eq!(event.schema, "airnote.mobile.event.v1");
    }

    #[test]
    fn vocab_fixture_contains_macobs() {
        let snapshot: VocabSnapshot = serde_json::from_str(include_str!(
            "../../../mobile/shared/fixtures/vocab_snapshot.json"
        ))
        .expect("vocab_snapshot fixture should parse");
        assert!(has_term(&snapshot, "Macobs"));
    }
}
