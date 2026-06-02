//! Wire types for the `GET /verifications` endpoint consumed by the
//! `chainlink-ccv` indexer's REST reader.
//!
//! Canonical shape: `chainlink-ccv/integration/pkg/api/v1/verifier_results.go`.
//! Reader: `chainlink-ccv/indexer/pkg/readers/rest_reader.go` (parses the body
//! with `json.Unmarshal` into `v1.VerifierResultsResponse`).
//!
//! The wire format is:
//!
//! ```json
//! {
//!   "results": [
//!     {
//!       "message":                  { ...22 protocol.Message fields, snake_case... },
//!       "message_ccv_addresses":    ["0x...", ...],
//!       "message_executor_address": "0x...",
//!       "ccv_data":                 "0x...",
//!       "metadata": {
//!         "timestamp":                1234567890,   // UnixMilli, JSON number
//!         "verifier_source_address":  "0x...",      // omitted if nil
//!         "verifier_dest_address":    "0x..."       // omitted if nil
//!       }
//!     }
//!   ],
//!   "errors": ["message not found: 0x..."]          // omitted if empty
//! }
//! ```
//!
//! All keys are snake_case. `errors`, `metadata`, and the two `metadata`
//! address fields use `omitempty` semantics. No `message_id` field — the
//! indexer derives the id from the message contents.

use serde::Serialize;

use super::ccip_message_v1::{HexBytes, MessageV1};

/// Top-level response envelope. Mirrors
/// `integration/pkg/api/v1/verifier_results.go::verifierResultsResponseJSON`.
#[derive(Debug, Serialize)]
pub struct VerifierResultsResponse {
    pub results: Vec<VerifierResult>,
    /// Per-input-index errors (e.g. "message not found: 0x..."). Position
    /// is informational only — the indexer ignores `errors` and re-keys
    /// `results` by `message.MessageID()`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

impl VerifierResultsResponse {
    pub fn empty() -> Self {
        Self {
            results: Vec::new(),
            errors: Vec::new(),
        }
    }
}

/// One verifier-attested result. Mirrors `verifierResultsJSON` upstream.
#[derive(Debug, Serialize)]
pub struct VerifierResult {
    pub message: MessageV1,
    pub message_ccv_addresses: Vec<HexBytes>,
    pub message_executor_address: HexBytes,
    pub ccv_data: HexBytes,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<VerifierResultMetadata>,
}

/// Verifier-side metadata. Mirrors `verifierResultsMetadataJSON` upstream.
///
/// `timestamp` is **UnixMilli** (i64), not seconds and not RFC3339 — the
/// upstream source field is `protocol.VerifierResult.Timestamp` (a `time.Time`)
/// and `NewVerifierResult` serializes it via `r.Timestamp.UnixMilli()`.
#[derive(Debug, Serialize)]
pub struct VerifierResultMetadata {
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifier_source_address: Option<HexBytes>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifier_dest_address: Option<HexBytes>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    //! Wire-format conformance tests.
    //!
    //! Fixtures are lifted verbatim from
    //! `chainlink-ccv/integration/pkg/api/v1/verifier_results_test.go`. If
    //! these fail, the indexer's `json.Unmarshal(body, &v1.VerifierResultsResponse)`
    //! will fail too — every drift surface here is one the indexer enforces.
    //!
    //! Strategy: build a Rust value, serialize to `serde_json::Value`, parse
    //! the canonical JSON fixture to `serde_json::Value`, compare structurally
    //! (order- and whitespace-insensitive — matches Go's `assert.JSONEq`).

    use super::*;
    use crate::provider::ccip_message_v1::{self, HexBytes, MessageV1};
    use alloy::primitives::B256;
    use serde_json::{Value, json};

    /// Asserts that the serialized Rust value matches the parsed expected JSON
    /// structurally. Pretty-prints both on failure so the diff is readable.
    fn assert_json_eq<S: Serialize>(actual: S, expected: &str) {
        let actual_v: Value = serde_json::to_value(&actual).expect("serialize");
        let expected_v: Value = serde_json::from_str(expected).expect("parse expected JSON");
        if actual_v != expected_v {
            panic!(
                "JSON mismatch\n--- actual ---\n{}\n--- expected ---\n{}\n",
                serde_json::to_string_pretty(&actual_v).unwrap(),
                serde_json::to_string_pretty(&expected_v).unwrap(),
            );
        }
    }

    /// Build a `MessageV1` matching the Go fixture at
    /// `verifier_results_test.go:213` (the "single result and no errors" case).
    ///
    /// We deliberately mirror the upstream test's quirk: length fields are 0
    /// even when the address byte slices are non-empty. Real wire traffic
    /// would have consistent lengths, but the upstream test author left them
    /// zero and our serialization must round-trip the same shape.
    fn fixture_message_single() -> MessageV1 {
        MessageV1 {
            version: 1,
            source_chain_selector: 100,
            dest_chain_selector: 200,
            sequence_number: 42,
            on_ramp_address: HexBytes::new(vec![0x01, 0x02, 0x03]),
            on_ramp_address_length: 0,
            off_ramp_address: HexBytes::new(vec![0x04, 0x05, 0x06]),
            off_ramp_address_length: 0,
            finality: 10,
            execution_gas_limit: 200_000,
            ccip_receive_gas_limit: 150_000,
            ccv_and_executor_hash: B256::ZERO,
            sender: HexBytes::new(vec![0x07, 0x08, 0x09]),
            sender_length: 0,
            receiver: HexBytes::new(vec![0x0a, 0x0b, 0x0c]),
            receiver_length: 0,
            dest_blob: HexBytes::new(vec![0x0d, 0x0e]),
            dest_blob_length: 0,
            token_transfer: None,
            token_transfer_length: 0,
            data: HexBytes::new(vec![0x10, 0x11]),
            data_length: 0,
        }
    }

    fn fixture_result_single() -> VerifierResult {
        VerifierResult {
            message: fixture_message_single(),
            message_ccv_addresses: vec![HexBytes::new(vec![0x13, 0x14, 0x15])],
            message_executor_address: HexBytes::new(vec![0x16, 0x17, 0x18]),
            ccv_data: HexBytes::new(vec![0x19, 0x1a, 0x1b]),
            metadata: Some(VerifierResultMetadata {
                timestamp: 1_234_567_890,
                verifier_source_address: Some(HexBytes::new(vec![0xa1, 0xa2])),
                verifier_dest_address: Some(HexBytes::new(vec![0xb1, 0xb2])),
            }),
        }
    }

    /// Canonical fixture from upstream `TestVerifierResultsResponse_RoundTrip`
    /// case "with single result and no errors".
    #[test]
    fn matches_canonical_single_result_no_errors() {
        let response = VerifierResultsResponse {
            results: vec![fixture_result_single()],
            errors: vec![],
        };
        let expected = r#"{
            "results": [{
                "message": {
                    "version": 1,
                    "source_chain_selector": 100,
                    "dest_chain_selector": 200,
                    "sequence_number": 42,
                    "on_ramp_address": "0x010203",
                    "on_ramp_address_length": 0,
                    "off_ramp_address": "0x040506",
                    "off_ramp_address_length": 0,
                    "finality": 10,
                    "execution_gas_limit": 200000,
                    "ccip_receive_gas_limit": 150000,
                    "ccv_and_executor_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "sender": "0x070809",
                    "sender_length": 0,
                    "receiver": "0x0a0b0c",
                    "receiver_length": 0,
                    "dest_blob": "0x0d0e",
                    "dest_blob_length": 0,
                    "token_transfer": null,
                    "token_transfer_length": 0,
                    "data": "0x1011",
                    "data_length": 0
                },
                "message_ccv_addresses": ["0x131415"],
                "message_executor_address": "0x161718",
                "ccv_data": "0x191a1b",
                "metadata": {
                    "timestamp": 1234567890,
                    "verifier_source_address": "0xa1a2",
                    "verifier_dest_address": "0xb1b2"
                }
            }]
        }"#;
        assert_json_eq(response, expected);
    }

    /// Canonical fixture from upstream `TestVerifierResultsResponse_RoundTrip`
    /// case "with multiple results and errors". Pins that:
    /// - `errors` is a top-level `[]string`
    /// - `results` ordering is preserved (positional)
    #[test]
    fn matches_canonical_multiple_results_with_errors() {
        let result1 = VerifierResult {
            message: MessageV1 {
                version: 1,
                source_chain_selector: 1,
                dest_chain_selector: 2,
                sequence_number: 10,
                on_ramp_address: HexBytes::new(vec![0x01]),
                on_ramp_address_length: 0,
                off_ramp_address: HexBytes::new(vec![0x02]),
                off_ramp_address_length: 0,
                finality: 5,
                execution_gas_limit: 100_000,
                ccip_receive_gas_limit: 50_000,
                ccv_and_executor_hash: B256::ZERO,
                sender: HexBytes::new(vec![0x03]),
                sender_length: 0,
                receiver: HexBytes::new(vec![0x04]),
                receiver_length: 0,
                dest_blob: HexBytes::new(vec![]),
                dest_blob_length: 0,
                token_transfer: None,
                token_transfer_length: 0,
                data: HexBytes::new(vec![]),
                data_length: 0,
            },
            message_ccv_addresses: vec![HexBytes::new(vec![0x05])],
            message_executor_address: HexBytes::new(vec![0x06]),
            ccv_data: HexBytes::new(vec![0x07]),
            metadata: Some(VerifierResultMetadata {
                timestamp: 9_999_999_999,
                verifier_source_address: Some(HexBytes::new(vec![0x11])),
                verifier_dest_address: Some(HexBytes::new(vec![0x22])),
            }),
        };
        let result2 = VerifierResult {
            message: MessageV1 {
                version: 2,
                source_chain_selector: 3,
                dest_chain_selector: 4,
                sequence_number: 20,
                on_ramp_address: HexBytes::new(vec![0xaa]),
                on_ramp_address_length: 0,
                off_ramp_address: HexBytes::new(vec![0xbb]),
                off_ramp_address_length: 0,
                finality: 15,
                execution_gas_limit: 300_000,
                ccip_receive_gas_limit: 250_000,
                ccv_and_executor_hash: B256::ZERO,
                sender: HexBytes::new(vec![0xcc]),
                sender_length: 0,
                receiver: HexBytes::new(vec![0xdd]),
                receiver_length: 0,
                dest_blob: HexBytes::new(vec![]),
                dest_blob_length: 0,
                token_transfer: None,
                token_transfer_length: 0,
                data: HexBytes::new(vec![]),
                data_length: 0,
            },
            message_ccv_addresses: vec![HexBytes::new(vec![0xee])],
            message_executor_address: HexBytes::new(vec![0xff]),
            ccv_data: HexBytes::new(vec![0x99]),
            metadata: Some(VerifierResultMetadata {
                timestamp: 8_888_888_888,
                verifier_source_address: Some(HexBytes::new(vec![0x33])),
                verifier_dest_address: Some(HexBytes::new(vec![0x44])),
            }),
        };
        let response = VerifierResultsResponse {
            results: vec![result1, result2],
            errors: vec!["error message 1".into(), "error message 2".into()],
        };

        let expected = r#"{
            "results": [
                {
                    "message": {
                        "version": 1,
                        "source_chain_selector": 1,
                        "dest_chain_selector": 2,
                        "sequence_number": 10,
                        "on_ramp_address": "0x01",
                        "on_ramp_address_length": 0,
                        "off_ramp_address": "0x02",
                        "off_ramp_address_length": 0,
                        "finality": 5,
                        "execution_gas_limit": 100000,
                        "ccip_receive_gas_limit": 50000,
                        "ccv_and_executor_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                        "sender": "0x03",
                        "sender_length": 0,
                        "receiver": "0x04",
                        "receiver_length": 0,
                        "dest_blob": "0x",
                        "dest_blob_length": 0,
                        "token_transfer": null,
                        "token_transfer_length": 0,
                        "data": "0x",
                        "data_length": 0
                    },
                    "message_ccv_addresses": ["0x05"],
                    "message_executor_address": "0x06",
                    "ccv_data": "0x07",
                    "metadata": {
                        "timestamp": 9999999999,
                        "verifier_source_address": "0x11",
                        "verifier_dest_address": "0x22"
                    }
                },
                {
                    "message": {
                        "version": 2,
                        "source_chain_selector": 3,
                        "dest_chain_selector": 4,
                        "sequence_number": 20,
                        "on_ramp_address": "0xaa",
                        "on_ramp_address_length": 0,
                        "off_ramp_address": "0xbb",
                        "off_ramp_address_length": 0,
                        "finality": 15,
                        "execution_gas_limit": 300000,
                        "ccip_receive_gas_limit": 250000,
                        "ccv_and_executor_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                        "sender": "0xcc",
                        "sender_length": 0,
                        "receiver": "0xdd",
                        "receiver_length": 0,
                        "dest_blob": "0x",
                        "dest_blob_length": 0,
                        "token_transfer": null,
                        "token_transfer_length": 0,
                        "data": "0x",
                        "data_length": 0
                    },
                    "message_ccv_addresses": ["0xee"],
                    "message_executor_address": "0xff",
                    "ccv_data": "0x99",
                    "metadata": {
                        "timestamp": 8888888888,
                        "verifier_source_address": "0x33",
                        "verifier_dest_address": "0x44"
                    }
                }
            ],
            "errors": ["error message 1", "error message 2"]
        }"#;
        assert_json_eq(response, expected);
    }

    /// Canonical fixture: "with empty results and errors" — both arrays empty.
    /// Upstream emits `{"results": []}` (errors omitted via `omitempty`).
    #[test]
    fn matches_canonical_empty_results_no_errors() {
        let response = VerifierResultsResponse::empty();
        assert_json_eq(response, r#"{"results": []}"#);
    }

    /// Canonical fixture: "with no results but with errors" — pins that
    /// `errors` IS emitted when populated even with empty `results`, and the
    /// 404 + populated-errors path produces the right wire shape.
    #[test]
    fn matches_canonical_no_results_with_errors() {
        let response = VerifierResultsResponse {
            results: vec![],
            errors: vec!["message not found".into()],
        };
        assert_json_eq(
            response,
            r#"{
                "results": [],
                "errors": ["message not found"]
            }"#,
        );
    }

    /// Canonical fixture from upstream `TestVerifierResultsMetadata_RoundTrip`
    /// case "with all fields populated".
    #[test]
    fn matches_canonical_metadata_all_fields() {
        let metadata = VerifierResultMetadata {
            timestamp: 1_234_567_890,
            verifier_source_address: Some(HexBytes::new(vec![0x01, 0x02, 0x03, 0x04, 0x05])),
            verifier_dest_address: Some(HexBytes::new(vec![0x06, 0x07, 0x08, 0x09, 0x0a])),
        };
        assert_json_eq(
            metadata,
            r#"{
                "timestamp": 1234567890,
                "verifier_source_address": "0x0102030405",
                "verifier_dest_address": "0x060708090a"
            }"#,
        );
    }

    /// Canonical fixture: "with empty addresses". Pins that nil addresses
    /// are OMITTED (not emitted as `"0x"`). This is the `omitempty` behavior
    /// from `verifierResultsMetadataJSON` and our `Option<HexBytes>` mirrors
    /// it via `skip_serializing_if = "Option::is_none"`.
    #[test]
    fn matches_canonical_metadata_omits_nil_addresses() {
        let metadata = VerifierResultMetadata {
            timestamp: 9_876_543_210,
            verifier_source_address: None,
            verifier_dest_address: None,
        };
        assert_json_eq(metadata, r#"{"timestamp": 9876543210}"#);
    }

    /// `metadata` field on `VerifierResult` has `omitempty` upstream — must
    /// be omitted when `None`. Pins `skip_serializing_if = "Option::is_none"`.
    #[test]
    fn result_omits_metadata_when_none() {
        let result = VerifierResult {
            message: fixture_message_single(),
            message_ccv_addresses: vec![HexBytes::new(vec![0x01])],
            message_executor_address: HexBytes::new(vec![0x02]),
            ccv_data: HexBytes::new(vec![0x03]),
            metadata: None,
        };
        let v: Value = serde_json::to_value(&result).unwrap();
        assert!(
            v.get("metadata").is_none(),
            "metadata key should be omitted when None, got: {}",
            v
        );
    }

    /// Hard guard against the previous PR's invented shape. If anyone ever
    /// reintroduces a `verifierResults` map wrapper or a `success` flag, this
    /// catches it before it ships.
    #[test]
    fn rejects_legacy_wrapper_fields() {
        let response = VerifierResultsResponse {
            results: vec![fixture_result_single()],
            errors: vec![],
        };
        let v: Value = serde_json::to_value(&response).unwrap();
        assert!(v.get("success").is_none(), "must not emit `success` field");
        assert!(
            v.get("verifierResults").is_none(),
            "must not emit camelCase `verifierResults` map"
        );
        // Top-level must be the canonical envelope.
        assert!(v.get("results").is_some(), "must emit `results` array");
        assert!(
            v.get("results").unwrap().is_array(),
            "`results` must be an array, not a map"
        );
    }

    /// Hard guard: the per-result object must NOT carry `message_id` (the
    /// indexer derives it from `message.MessageID()`), and must NOT use the
    /// camelCase wrapper `verifierResult`/`verifierName`/etc.
    #[test]
    fn rejects_legacy_per_result_fields() {
        let result = fixture_result_single();
        let v: Value = serde_json::to_value(&result).unwrap();
        for forbidden in [
            "message_id",
            "verifierResult",
            "verifierName",
            "attestationTimestamp",
            "ingestionTimestamp",
            "verifier_source_address", // belongs on metadata, not on VerifierResult
            "verifier_dest_address",
        ] {
            assert!(
                v.get(forbidden).is_none(),
                "VerifierResult must not emit `{}` at top level, got: {}",
                forbidden,
                v
            );
        }
    }

    /// `metadata.timestamp` must be an unquoted JSON integer (UnixMilli i64),
    /// not an RFC3339 string. The indexer's protobuf-backed unmarshal parses
    /// it as `int64`; a string fails silently with a JSON unmarshal error.
    #[test]
    fn metadata_timestamp_is_unquoted_integer() {
        let metadata = VerifierResultMetadata {
            timestamp: 1_700_000_000_123,
            verifier_source_address: None,
            verifier_dest_address: None,
        };
        let json = serde_json::to_string(&metadata).unwrap();
        assert!(
            json.contains("\"timestamp\":1700000000123"),
            "timestamp must serialize as unquoted i64, got: {}",
            json
        );
    }

    /// Sanity: the `MessageV1` decoder produces a round-trippable JSON shape
    /// when fed a synthetic header. Confirms that re-using the existing
    /// `ccip_message_v1::decode` is wire-compatible with the canonical
    /// envelope (the protocol.Message subtree).
    #[test]
    fn decoded_messagev1_roundtrips_through_envelope() {
        // Minimal valid MessageV1 byte stream (no dynamic fields).
        let mut bytes = Vec::with_capacity(79);
        bytes.push(1u8); // version
        bytes.extend_from_slice(&1u64.to_be_bytes()); // source_chain_selector
        bytes.extend_from_slice(&2u64.to_be_bytes()); // dest_chain_selector
        bytes.extend_from_slice(&3u64.to_be_bytes()); // sequence_number
        bytes.extend_from_slice(&0u32.to_be_bytes()); // execution_gas_limit
        bytes.extend_from_slice(&0u32.to_be_bytes()); // ccip_receive_gas_limit
        bytes.extend_from_slice(&0u32.to_be_bytes()); // finality
        bytes.extend_from_slice(&[0u8; 32]); // ccv_and_executor_hash
        bytes.push(0); // on_ramp_address_length
        bytes.push(0); // off_ramp_address_length
        bytes.push(0); // sender_length
        bytes.push(0); // receiver_length
        bytes.extend_from_slice(&0u16.to_be_bytes()); // dest_blob_length
        bytes.extend_from_slice(&0u16.to_be_bytes()); // token_transfer_length
        bytes.extend_from_slice(&0u16.to_be_bytes()); // data_length

        let msg = ccip_message_v1::decode(&bytes).unwrap();
        let result = VerifierResult {
            message: msg,
            message_ccv_addresses: vec![],
            message_executor_address: HexBytes::new(vec![]),
            ccv_data: HexBytes::new(vec![]),
            metadata: None,
        };
        let v: Value = serde_json::to_value(&result).unwrap();
        // Spot-check the nested message object has all 22 expected snake_case keys.
        let msg = v.get("message").expect("message key present");
        for key in [
            "version",
            "source_chain_selector",
            "dest_chain_selector",
            "sequence_number",
            "on_ramp_address",
            "on_ramp_address_length",
            "off_ramp_address",
            "off_ramp_address_length",
            "finality",
            "execution_gas_limit",
            "ccip_receive_gas_limit",
            "ccv_and_executor_hash",
            "sender",
            "sender_length",
            "receiver",
            "receiver_length",
            "dest_blob",
            "dest_blob_length",
            "token_transfer",
            "token_transfer_length",
            "data",
            "data_length",
        ] {
            assert!(msg.get(key).is_some(), "missing `message.{}` in {}", key, v);
        }
        // Empty byte slices serialize as "0x" not null.
        assert_eq!(v.get("message_executor_address").unwrap(), &json!("0x"));
        assert_eq!(v.get("ccv_data").unwrap(), &json!("0x"));
    }
}
