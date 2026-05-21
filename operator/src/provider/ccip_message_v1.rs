//! Decoder for CCIP v1.7 `MessageV1` packed wire format.
//!
//! Mirrors `chainlink-ccv/protocol/message_types.go` `DecodeMessage` /
//! `DecodeTokenTransfer`. The packed format is what the source-chain `OnRamp`
//! emits in the `CCIPMessageSent` event's `encodedMessage` field. We need a
//! full Rust decoder (not just the static header in `parse_ccip_receive_gas_limit`)
//! so the operator can populate the `Message` sub-struct served by the
//! `GET /verifications` endpoint.
//!
//! Field order and lengths must match the Go source exactly — the indexer
//! re-encodes and re-hashes the message to verify `MessageID`, so any drift
//! produces a silent indexer-side validation failure.
//!
//! JSON serialization mirrors the Go struct's `json:` tags (snake_case).

use alloy::primitives::{B256, U256};
use serde::Serialize;

use crate::error::ProviderError;

/// Minimum size in bytes of the static header (matches `MinSizeRequiredMsgFields = 79` upstream).
/// Static layout up through `ccv_and_executor_hash` is bytes 0..69; the four 1-byte length
/// prefixes for on-ramp/off-ramp/sender/receiver plus three u16 length prefixes for
/// dest-blob/token-transfer/data add up to 79 minimum even with all dynamic fields empty.
const MIN_MESSAGE_BYTES: usize = 79;

/// Chain-agnostic CCIP message — Rust mirror of upstream Go `protocol.Message`.
#[derive(Debug, Clone, Serialize)]
pub struct MessageV1 {
    pub sender: HexBytes,
    pub data: HexBytes,
    pub on_ramp_address: HexBytes,
    /// Null when no token transfer is present (must serialize as JSON `null`, not be omitted).
    pub token_transfer: Option<TokenTransfer>,
    pub off_ramp_address: HexBytes,
    pub dest_blob: HexBytes,
    pub receiver: HexBytes,
    pub source_chain_selector: u64,
    pub dest_chain_selector: u64,
    pub sequence_number: u64,
    pub execution_gas_limit: u32,
    pub ccip_receive_gas_limit: u32,
    pub finality: u32,
    pub ccv_and_executor_hash: B256,
    pub dest_blob_length: u16,
    pub token_transfer_length: u16,
    pub data_length: u16,
    pub receiver_length: u8,
    pub sender_length: u8,
    pub version: u8,
    pub off_ramp_address_length: u8,
    pub on_ramp_address_length: u8,
}

/// Embedded token transfer when the message carries one.
#[derive(Debug, Clone, Serialize)]
pub struct TokenTransfer {
    /// 256-bit unsigned amount. Serialized as a JSON number via
    /// `arbitrary_precision` to match Go `*big.Int` wire encoding.
    #[serde(serialize_with = "serialize_u256_as_json_number")]
    pub amount: U256,
    pub source_pool_address: HexBytes,
    pub source_token_address: HexBytes,
    pub dest_token_address: HexBytes,
    pub token_receiver: HexBytes,
    pub extra_data: HexBytes,
    pub version: u8,
    pub source_pool_address_length: u8,
    pub source_token_address_length: u8,
    pub dest_token_address_length: u8,
    pub token_receiver_length: u8,
    pub extra_data_length: u16,
}

/// Byte slice that serializes as `"0x<lowercase-hex>"` to match Go's
/// `UnknownAddress` / `ByteSlice` JSON encoding. Empty bytes serialize as `"0x"`.
#[derive(Debug, Clone, Default)]
pub struct HexBytes(pub Vec<u8>);

impl HexBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    #[allow(dead_code)]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.0.len()
    }
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for HexBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("0x{}", hex::encode(&self.0)))
    }
}

/// Serialize a `U256` as a JSON number (not a string), matching Go's
/// `*big.Int` default JSON encoding. Requires `serde_json` with the
/// `arbitrary_precision` feature; without it large values lose precision.
fn serialize_u256_as_json_number<S: serde::Serializer>(v: &U256, s: S) -> Result<S::Ok, S::Error> {
    // serde_json's Number with arbitrary_precision parses decimal strings of any width.
    let decimal = v.to_string();
    let number: serde_json::Number = decimal
        .parse()
        .map_err(<S::Error as serde::ser::Error>::custom)?;
    number.serialize(s)
}

/// Decode a CCIP v1.7 packed `MessageV1` byte stream. Strict: rejects trailing bytes.
pub fn decode(data: &[u8]) -> Result<MessageV1, ProviderError> {
    if data.len() < MIN_MESSAGE_BYTES {
        return Err(ProviderError::EventDecode(format!(
            "message too short: {} < {} bytes",
            data.len(),
            MIN_MESSAGE_BYTES
        )));
    }

    let mut cur = Cursor::new(data);
    let version = cur.read_u8("version")?;
    let source_chain_selector = cur.read_u64_be("source_chain_selector")?;
    let dest_chain_selector = cur.read_u64_be("dest_chain_selector")?;
    let sequence_number = cur.read_u64_be("sequence_number")?;
    let execution_gas_limit = cur.read_u32_be("execution_gas_limit")?;
    let ccip_receive_gas_limit = cur.read_u32_be("ccip_receive_gas_limit")?;
    let finality = cur.read_u32_be("finality")?;
    let ccv_and_executor_hash = B256::from(cur.read_array::<32>("ccv_and_executor_hash")?);

    let on_ramp_address_length = cur.read_u8("on_ramp_address_length")?;
    let on_ramp_address =
        HexBytes::new(cur.read_bytes(on_ramp_address_length as usize, "on_ramp_address")?);

    let off_ramp_address_length = cur.read_u8("off_ramp_address_length")?;
    let off_ramp_address =
        HexBytes::new(cur.read_bytes(off_ramp_address_length as usize, "off_ramp_address")?);

    let sender_length = cur.read_u8("sender_length")?;
    let sender = HexBytes::new(cur.read_bytes(sender_length as usize, "sender")?);

    let receiver_length = cur.read_u8("receiver_length")?;
    let receiver = HexBytes::new(cur.read_bytes(receiver_length as usize, "receiver")?);

    let dest_blob_length = cur.read_u16_be("dest_blob_length")?;
    let dest_blob = HexBytes::new(cur.read_bytes(dest_blob_length as usize, "dest_blob")?);

    let token_transfer_length = cur.read_u16_be("token_transfer_length")?;
    let token_transfer = if token_transfer_length == 0 {
        None
    } else {
        let raw = cur.read_bytes(token_transfer_length as usize, "token_transfer")?;
        Some(decode_token_transfer(&raw)?)
    };

    let data_length = cur.read_u16_be("data_length")?;
    let data = HexBytes::new(cur.read_bytes(data_length as usize, "data")?);

    if !cur.is_empty() {
        return Err(ProviderError::EventDecode(format!(
            "trailing {} bytes after message decode",
            cur.remaining()
        )));
    }

    Ok(MessageV1 {
        sender,
        data,
        on_ramp_address,
        token_transfer,
        off_ramp_address,
        dest_blob,
        receiver,
        source_chain_selector,
        dest_chain_selector,
        sequence_number,
        execution_gas_limit,
        ccip_receive_gas_limit,
        finality,
        ccv_and_executor_hash,
        dest_blob_length,
        token_transfer_length,
        data_length,
        receiver_length,
        sender_length,
        version,
        off_ramp_address_length,
        on_ramp_address_length,
    })
}

fn decode_token_transfer(data: &[u8]) -> Result<TokenTransfer, ProviderError> {
    // Minimum: 1 (version) + 32 (amount) + 4 (one-byte length prefixes) + 2 (extra_data_length) = 39
    const MIN_TT_BYTES: usize = 39;
    if data.len() < MIN_TT_BYTES {
        return Err(ProviderError::EventDecode(format!(
            "token transfer too short: {} < {} bytes",
            data.len(),
            MIN_TT_BYTES
        )));
    }

    let mut cur = Cursor::new(data);
    let version = cur.read_u8("token_transfer.version")?;
    let amount_bytes: [u8; 32] = cur.read_array::<32>("token_transfer.amount")?;
    let amount = U256::from_be_bytes(amount_bytes);

    let source_pool_address_length = cur.read_u8("source_pool_address_length")?;
    let source_pool_address =
        HexBytes::new(cur.read_bytes(source_pool_address_length as usize, "source_pool_address")?);

    let source_token_address_length = cur.read_u8("source_token_address_length")?;
    let source_token_address = HexBytes::new(
        cur.read_bytes(source_token_address_length as usize, "source_token_address")?,
    );

    let dest_token_address_length = cur.read_u8("dest_token_address_length")?;
    let dest_token_address =
        HexBytes::new(cur.read_bytes(dest_token_address_length as usize, "dest_token_address")?);

    let token_receiver_length = cur.read_u8("token_receiver_length")?;
    let token_receiver =
        HexBytes::new(cur.read_bytes(token_receiver_length as usize, "token_receiver")?);

    let extra_data_length = cur.read_u16_be("extra_data_length")?;
    let extra_data = HexBytes::new(cur.read_bytes(extra_data_length as usize, "extra_data")?);

    if !cur.is_empty() {
        return Err(ProviderError::EventDecode(format!(
            "trailing {} bytes after token transfer decode",
            cur.remaining()
        )));
    }

    Ok(TokenTransfer {
        amount,
        source_pool_address,
        source_token_address,
        dest_token_address,
        token_receiver,
        extra_data,
        version,
        source_pool_address_length,
        source_token_address_length,
        dest_token_address_length,
        token_receiver_length,
        extra_data_length,
    })
}

/// Minimal byte-stream reader. Avoids pulling in `byteorder`/`bytes` for one consumer.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }
    fn require(&self, n: usize, field: &str) -> Result<(), ProviderError> {
        if self.remaining() < n {
            return Err(ProviderError::EventDecode(format!(
                "buffer underflow reading {}: need {} bytes, have {}",
                field,
                n,
                self.remaining()
            )));
        }
        Ok(())
    }
    fn read_u8(&mut self, field: &str) -> Result<u8, ProviderError> {
        self.require(1, field)?;
        let v = self.bytes[self.pos];
        self.pos += 1;
        Ok(v)
    }
    fn read_u16_be(&mut self, field: &str) -> Result<u16, ProviderError> {
        Ok(u16::from_be_bytes(self.read_array::<2>(field)?))
    }
    fn read_u32_be(&mut self, field: &str) -> Result<u32, ProviderError> {
        Ok(u32::from_be_bytes(self.read_array::<4>(field)?))
    }
    fn read_u64_be(&mut self, field: &str) -> Result<u64, ProviderError> {
        Ok(u64::from_be_bytes(self.read_array::<8>(field)?))
    }
    fn read_array<const N: usize>(&mut self, field: &str) -> Result<[u8; N], ProviderError> {
        self.require(N, field)?;
        let mut buf = [0u8; N];
        buf.copy_from_slice(&self.bytes[self.pos..self.pos + N]);
        self.pos += N;
        Ok(buf)
    }
    fn read_bytes(&mut self, n: usize, field: &str) -> Result<Vec<u8>, ProviderError> {
        self.require(n, field)?;
        let v = self.bytes[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(v)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Build a minimal valid MessageV1 byte stream with no dynamic fields.
    fn minimal_static_header(version: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MIN_MESSAGE_BYTES);
        buf.push(version); // version
        buf.extend_from_slice(&1u64.to_be_bytes()); // source_chain_selector
        buf.extend_from_slice(&2u64.to_be_bytes()); // dest_chain_selector
        buf.extend_from_slice(&3u64.to_be_bytes()); // sequence_number
        buf.extend_from_slice(&100_000u32.to_be_bytes()); // execution_gas_limit
        buf.extend_from_slice(&200_000u32.to_be_bytes()); // ccip_receive_gas_limit
        buf.extend_from_slice(&0u32.to_be_bytes()); // finality
        buf.extend_from_slice(&[0xAB; 32]); // ccv_and_executor_hash
        buf.push(0); // on_ramp_address_length
        buf.push(0); // off_ramp_address_length
        buf.push(0); // sender_length
        buf.push(0); // receiver_length
        buf.extend_from_slice(&0u16.to_be_bytes()); // dest_blob_length
        buf.extend_from_slice(&0u16.to_be_bytes()); // token_transfer_length
        buf.extend_from_slice(&0u16.to_be_bytes()); // data_length
        buf
    }

    #[test]
    fn test_decode_minimal_message() {
        let bytes = minimal_static_header(1);
        assert_eq!(bytes.len(), MIN_MESSAGE_BYTES);
        let msg = decode(&bytes).unwrap();
        assert_eq!(msg.version, 1);
        assert_eq!(msg.source_chain_selector, 1);
        assert_eq!(msg.dest_chain_selector, 2);
        assert_eq!(msg.sequence_number, 3);
        assert_eq!(msg.execution_gas_limit, 100_000);
        assert_eq!(msg.ccip_receive_gas_limit, 200_000);
        assert_eq!(msg.finality, 0);
        assert_eq!(msg.ccv_and_executor_hash, B256::from([0xAB; 32]));
        assert!(msg.sender.is_empty());
        assert!(msg.receiver.is_empty());
        assert!(msg.data.is_empty());
        assert!(msg.token_transfer.is_none());
    }

    #[test]
    fn test_decode_with_dynamic_fields() {
        let mut buf = Vec::new();
        buf.push(1u8); // version
        buf.extend_from_slice(&0x1234_5678_9abc_def0u64.to_be_bytes()); // source
        buf.extend_from_slice(&0x0fed_cba9_8765_4321u64.to_be_bytes()); // dest
        buf.extend_from_slice(&42u64.to_be_bytes()); // seq
        buf.extend_from_slice(&50_000u32.to_be_bytes()); // exec gas
        buf.extend_from_slice(&75_000u32.to_be_bytes()); // recv gas
        buf.extend_from_slice(&[0u8; 4]); // finality
        buf.extend_from_slice(&[0u8; 32]); // ccv hash

        let on_ramp = vec![0x11u8; 20];
        buf.push(on_ramp.len() as u8);
        buf.extend_from_slice(&on_ramp);

        let off_ramp = vec![0x22u8; 20];
        buf.push(off_ramp.len() as u8);
        buf.extend_from_slice(&off_ramp);

        let sender = vec![0x33u8; 20];
        buf.push(sender.len() as u8);
        buf.extend_from_slice(&sender);

        let receiver = vec![0x44u8; 32];
        buf.push(receiver.len() as u8);
        buf.extend_from_slice(&receiver);

        buf.extend_from_slice(&0u16.to_be_bytes()); // dest_blob empty
        buf.extend_from_slice(&0u16.to_be_bytes()); // token_transfer empty

        let data = b"hello, ccip".to_vec();
        buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
        buf.extend_from_slice(&data);

        let msg = decode(&buf).unwrap();
        assert_eq!(msg.source_chain_selector, 0x1234_5678_9abc_def0);
        assert_eq!(msg.dest_chain_selector, 0x0fed_cba9_8765_4321);
        assert_eq!(msg.sequence_number, 42);
        assert_eq!(msg.on_ramp_address.as_slice(), on_ramp.as_slice());
        assert_eq!(msg.off_ramp_address.as_slice(), off_ramp.as_slice());
        assert_eq!(msg.sender.as_slice(), sender.as_slice());
        assert_eq!(msg.receiver.as_slice(), receiver.as_slice());
        assert_eq!(msg.data.as_slice(), data.as_slice());
        assert_eq!(msg.data_length, data.len() as u16);
        assert!(msg.token_transfer.is_none());
    }

    #[test]
    fn test_decode_with_token_transfer() {
        // Build a valid TokenTransfer payload.
        let mut tt = Vec::new();
        tt.push(1u8); // version
        let amount = U256::from(1_000_000_000_000_000_000u128); // 1 ETH
        tt.extend_from_slice(&amount.to_be_bytes::<32>());
        let src_pool = vec![0xAAu8; 20];
        tt.push(src_pool.len() as u8);
        tt.extend_from_slice(&src_pool);
        let src_tok = vec![0xBBu8; 20];
        tt.push(src_tok.len() as u8);
        tt.extend_from_slice(&src_tok);
        let dst_tok = vec![0xCCu8; 20];
        tt.push(dst_tok.len() as u8);
        tt.extend_from_slice(&dst_tok);
        let receiver = vec![0xDDu8; 32];
        tt.push(receiver.len() as u8);
        tt.extend_from_slice(&receiver);
        let extra = b"extra".to_vec();
        tt.extend_from_slice(&(extra.len() as u16).to_be_bytes());
        tt.extend_from_slice(&extra);

        let mut msg = minimal_static_header(1);
        // Patch the token_transfer_length (offset = 69 static + 4 zero-length prefixes + 2 dest_blob = 75)
        let tt_len_offset = 75;
        msg.splice(
            tt_len_offset..tt_len_offset + 2,
            (tt.len() as u16).to_be_bytes(),
        );
        // Insert the token transfer bytes BEFORE the data_length u16.
        msg.splice(tt_len_offset + 2..tt_len_offset + 2, tt.iter().copied());

        let decoded = decode(&msg).unwrap();
        let token = decoded.token_transfer.expect("expected token transfer");
        assert_eq!(token.amount, amount);
        assert_eq!(token.source_pool_address.as_slice(), src_pool.as_slice());
        assert_eq!(token.source_token_address.as_slice(), src_tok.as_slice());
        assert_eq!(token.dest_token_address.as_slice(), dst_tok.as_slice());
        assert_eq!(token.token_receiver.as_slice(), receiver.as_slice());
        assert_eq!(token.extra_data.as_slice(), extra.as_slice());
    }

    #[test]
    fn test_decode_rejects_short_buffer() {
        let buf = vec![0u8; 10];
        let err = decode(&buf).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn test_decode_rejects_trailing_bytes() {
        let mut bytes = minimal_static_header(1);
        bytes.push(0xFFu8);
        let err = decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("trailing"));
    }

    #[test]
    fn test_hexbytes_serialize_with_prefix_and_lowercase() {
        let h = HexBytes::new(vec![0xAB, 0xCD, 0xEF, 0x01]);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"0xabcdef01\"");
    }

    #[test]
    fn test_hexbytes_empty_serializes_as_0x() {
        let h = HexBytes::new(vec![]);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"0x\"");
    }

    #[test]
    fn test_token_transfer_amount_serializes_as_json_number() {
        let tt = TokenTransfer {
            amount: U256::from(1_000_000_000_000_000_000u128),
            source_pool_address: HexBytes::default(),
            source_token_address: HexBytes::default(),
            dest_token_address: HexBytes::default(),
            token_receiver: HexBytes::default(),
            extra_data: HexBytes::default(),
            version: 1,
            source_pool_address_length: 0,
            source_token_address_length: 0,
            dest_token_address_length: 0,
            token_receiver_length: 0,
            extra_data_length: 0,
        };
        let json = serde_json::to_string(&tt).unwrap();
        // Amount must be an unquoted JSON number to match Go *big.Int wire encoding.
        assert!(
            json.contains("\"amount\":1000000000000000000"),
            "expected unquoted amount, got: {}",
            json
        );
    }

    #[test]
    fn test_message_json_field_names_snake_case() {
        let msg = decode(&minimal_static_header(1)).unwrap();
        let json = serde_json::to_string(&msg).unwrap();
        for field in [
            "\"sender\"",
            "\"data\"",
            "\"on_ramp_address\"",
            "\"token_transfer\"",
            "\"off_ramp_address\"",
            "\"dest_blob\"",
            "\"receiver\"",
            "\"source_chain_selector\"",
            "\"dest_chain_selector\"",
            "\"sequence_number\"",
            "\"execution_gas_limit\"",
            "\"ccip_receive_gas_limit\"",
            "\"finality\"",
            "\"ccv_and_executor_hash\"",
            "\"dest_blob_length\"",
            "\"token_transfer_length\"",
            "\"data_length\"",
            "\"receiver_length\"",
            "\"sender_length\"",
            "\"version\"",
            "\"off_ramp_address_length\"",
            "\"on_ramp_address_length\"",
        ] {
            assert!(
                json.contains(field),
                "missing field {} in JSON: {}",
                field,
                json
            );
        }
        // token_transfer must be null when absent (not omitted).
        assert!(json.contains("\"token_transfer\":null"), "json: {}", json);
    }

    #[test]
    fn test_round_trip_parse_ccip_receive_gas_limit_offset() {
        // Cross-check: our new decoder must produce the same ccip_receive_gas_limit
        // as the existing parse_ccip_receive_gas_limit helper at byte offset 29.
        let bytes = minimal_static_header(1);
        let msg = decode(&bytes).unwrap();
        assert_eq!(msg.ccip_receive_gas_limit, 200_000);
    }
}
