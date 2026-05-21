//! Wire types for the `GET /verifications` endpoint consumed by the
//! `chainlink-ccv` indexer's REST reader.
//!
//! Shape from upstream `indexer/pkg/api/handlers/v1/verifier_results.go` and
//! `indexer/pkg/common/metadata.go`. Deliberately stricter than the single-map
//! shape the public design doc shows — see
//! `devdocs/chainlink-executor-api-integration.md` notes for the divergence.
//!
//! Casing on the wire is mixed and intentional:
//!
//! - Outer wrapper and `metadata` block: camelCase (`verifierResults`, `verifierName`).
//! - Inner `verifierResult` and `MessageV1`: snake_case (`message_id`, `ccv_data`,
//!   `source_chain_selector`).
//!
//! Wrong casing produces a silent `json.Unmarshal` failure indexer-side with
//! no HTTP error visible from our end — keep the field-name guards in
//! `tests` honest.

use std::collections::BTreeMap;

use serde::Serialize;

use super::ccip_message_v1::{HexBytes, MessageV1};

/// Top-level response wrapper. Maps message id (`"0x<lowercase 64-hex>"`) to a
/// list of verifier results (one per attesting operator/run).
#[derive(Debug, Serialize)]
pub struct VerifierResultsResponse {
    pub success: bool,
    /// Map keyed by lowercase `0x`-prefixed messageID. Use a BTreeMap so the
    /// JSON ordering is deterministic for byte-comparable golden tests.
    #[serde(rename = "verifierResults")]
    pub verifier_results: BTreeMap<String, Vec<VerifierResultWithMetadata>>,
}

impl VerifierResultsResponse {
    pub fn empty() -> Self {
        Self {
            success: true,
            verifier_results: BTreeMap::new(),
        }
    }
}

/// Single entry: the result itself + indexer-facing metadata.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifierResultWithMetadata {
    pub verifier_result: VerifierResult,
    pub metadata: VerifierResultMetadata,
}

/// Indexer metadata. Matches upstream `indexer/pkg/common/metadata.go`.
/// All three timestamps are RFC3339Nano strings (Go `time.Time` default).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifierResultMetadata {
    pub verifier_name: String,
    pub attestation_timestamp: String,
    pub ingestion_timestamp: String,
}

/// The verifier result itself. Matches upstream `protocol/message_types.go`
/// `VerifierResult` with snake_case JSON tags.
#[derive(Debug, Serialize)]
pub struct VerifierResult {
    pub message_id: String,
    pub message: MessageV1,
    pub message_ccv_addresses: Vec<String>,
    pub message_executor_address: String,
    pub ccv_data: HexBytes,
    pub timestamp: String,
    pub verifier_source_address: String,
    pub verifier_dest_address: String,
}

/// Format Unix seconds as RFC3339Nano (`YYYY-MM-DDTHH:MM:SS.000000000Z`),
/// matching Go's `time.Time` default JSON encoding. Stored attestation
/// timestamps are seconds-precision; subsecond fields are zero-padded.
pub fn rfc3339_nano_from_unix_seconds(secs: u64) -> String {
    use chrono::{DateTime, SecondsFormat, Utc};
    let dt: DateTime<Utc> = DateTime::<Utc>::from_timestamp(secs as i64, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is valid"));
    dt.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn dummy_message() -> MessageV1 {
        let bytes = {
            let mut buf = Vec::with_capacity(79);
            buf.push(1u8); // version
            buf.extend_from_slice(&1u64.to_be_bytes());
            buf.extend_from_slice(&2u64.to_be_bytes());
            buf.extend_from_slice(&3u64.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&[0u8; 32]);
            buf.push(0); // on_ramp_address_length
            buf.push(0); // off_ramp_address_length
            buf.push(0); // sender_length
            buf.push(0); // receiver_length
            buf.extend_from_slice(&0u16.to_be_bytes()); // dest_blob_length
            buf.extend_from_slice(&0u16.to_be_bytes()); // token_transfer_length
            buf.extend_from_slice(&0u16.to_be_bytes()); // data_length
            buf
        };
        super::super::ccip_message_v1::decode(&bytes).unwrap()
    }

    fn dummy_result() -> VerifierResultWithMetadata {
        VerifierResultWithMetadata {
            verifier_result: VerifierResult {
                message_id: "0xaa".to_string() + &"00".repeat(31),
                message: dummy_message(),
                message_ccv_addresses: vec!["0x2222222222222222222222222222222222222222".into()],
                message_executor_address: "0x3333333333333333333333333333333333333333".into(),
                ccv_data: HexBytes::new(vec![0xCC, 0xDD]),
                timestamp: rfc3339_nano_from_unix_seconds(1_700_000_000),
                verifier_source_address: "0x4444444444444444444444444444444444444444".into(),
                verifier_dest_address: "0x5555555555555555555555555555555555555555".into(),
            },
            metadata: VerifierResultMetadata {
                verifier_name: "symbiotic-ccv".into(),
                attestation_timestamp: rfc3339_nano_from_unix_seconds(1_700_000_000),
                ingestion_timestamp: rfc3339_nano_from_unix_seconds(1_700_000_001),
            },
        }
    }

    #[test]
    fn test_empty_response_shape() {
        let resp = VerifierResultsResponse::empty();
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"success":true,"verifierResults":{}}"#);
    }

    #[test]
    fn test_wrapper_uses_camelcase_verifierresults() {
        let mut resp = VerifierResultsResponse::empty();
        let id = format!("0x{}", "aa".repeat(32));
        resp.verifier_results
            .insert(id.clone(), vec![dummy_result()]);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"verifierResults\":"), "json: {}", json);
        assert!(json.contains(&format!("\"{}\":", id)));
    }

    #[test]
    fn test_metadata_uses_camelcase_field_names() {
        let result = dummy_result();
        let json = serde_json::to_string(&result).unwrap();
        for field in [
            "\"verifierResult\"",
            "\"metadata\"",
            "\"verifierName\"",
            "\"attestationTimestamp\"",
            "\"ingestionTimestamp\"",
        ] {
            assert!(json.contains(field), "missing {} in {}", field, json);
        }
    }

    #[test]
    fn test_verifier_result_uses_snake_case() {
        let result = dummy_result();
        let json = serde_json::to_string(&result.verifier_result).unwrap();
        for field in [
            "\"message_id\"",
            "\"message\"",
            "\"message_ccv_addresses\"",
            "\"message_executor_address\"",
            "\"ccv_data\"",
            "\"timestamp\"",
            "\"verifier_source_address\"",
            "\"verifier_dest_address\"",
        ] {
            assert!(
                field_in_object(&json, field),
                "missing {} in {}",
                field,
                json
            );
        }
    }

    /// Crude check that a field name appears as a top-level key in the JSON
    /// object, not just as substring inside a nested struct.
    fn field_in_object(json: &str, field: &str) -> bool {
        json.contains(&format!("{}:", field))
    }

    #[test]
    fn test_ccv_data_serializes_as_hex_string() {
        let result = dummy_result();
        let json: Value = serde_json::to_value(&result.verifier_result).unwrap();
        assert_eq!(json.get("ccv_data").unwrap().as_str().unwrap(), "0xccdd");
    }

    #[test]
    fn test_rfc3339_nano_format() {
        let s = rfc3339_nano_from_unix_seconds(1_700_000_000);
        // Format: 2023-11-14T22:13:20.000000000Z (nanos zero-padded, trailing Z)
        assert!(s.ends_with('Z'), "expected trailing Z, got {}", s);
        assert!(s.contains('.'), "expected nanos separator, got {}", s);
        // The integer-seconds round-trips as 2023-11-14T22:13:20.000000000Z
        assert_eq!(s, "2023-11-14T22:13:20.000000000Z");
    }
}
