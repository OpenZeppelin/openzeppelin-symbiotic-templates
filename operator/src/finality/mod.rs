//! Source-chain finality gating for CCIP CCV messages.
//!
//! A CCIP message carries a `finality` value (the `bytes4` field at offset 33 of
//! the packed MessageV1) that specifies how final the source transaction must be
//! before a verifier should attest to it. We honour it by deferring signing until
//! the source block has reached the requested finality.
//!
//! Wire format (mirrors `chainlink-ccv/protocol/finality.go`):
//!
//! ```text
//!   Bit: 31..16 | 15..0
//!       +-------+-------+
//!       | flags | depth |
//!       +-------+-------+
//! ```
//!
//! * `0x00000000` — wait for full finality (the `finalized` tag). Default, safest.
//! * bit 16 set  — wait for the `safe` head.
//! * `0x0001..0xFFFF` (depth, no flags) — wait for N block confirmations.

use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::BlockTransactionsKind;
use alloy::transports::http::{Client, Http};
use async_trait::async_trait;
use tokio::sync::Mutex;

/// Bit 16 — wait for the `safe` head (`FinalityWaitForSafe`).
const FINALITY_FLAG_SAFE: u32 = 0x0001_0000;
/// Lower 16 bits hold the confirmation depth.
const FINALITY_DEPTH_MASK: u32 = 0x0000_FFFF;

/// How long a fetched [`SourceHead`] is reused before refetching. Keeps the
/// per-message checks in a single sign-job tick down to one RPC round-trip.
const HEAD_CACHE_TTL: Duration = Duration::from_secs(2);

/// The finality requirement decoded from a message's `finality` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalityRequirement {
    /// Wait for the source block to be `finalized` (default, `finality == 0`).
    Finalized,
    /// Wait for the source block to reach the `safe` head.
    Safe,
    /// Wait for `n` block confirmations (capped at finalization downstream).
    Confirmations(u32),
}

/// Decode a raw `finality` u32 into its semantic requirement.
pub fn parse_finality(raw: u32) -> FinalityRequirement {
    if raw & FINALITY_FLAG_SAFE != 0 {
        return FinalityRequirement::Safe;
    }
    let depth = raw & FINALITY_DEPTH_MASK;
    if depth == 0 {
        FinalityRequirement::Finalized
    } else {
        FinalityRequirement::Confirmations(depth)
    }
}

/// Current source-chain head positions.
#[derive(Debug, Clone, Copy)]
pub struct SourceHead {
    pub latest: u64,
    pub finalized: u64,
    pub safe: u64,
}

/// Whether a message at `msg_block` satisfies `req` given the current `head`.
///
/// Confirmation depth is OR-ed with finalization (per the upstream verifier),
/// which caps an unreasonably high custom depth at finality and prevents it
/// from wedging a message forever.
pub fn is_ready(req: &FinalityRequirement, msg_block: u64, head: &SourceHead) -> bool {
    match req {
        FinalityRequirement::Finalized => msg_block <= head.finalized,
        FinalityRequirement::Safe => msg_block <= head.safe,
        FinalityRequirement::Confirmations(n) => {
            msg_block.saturating_add(u64::from(*n)) <= head.latest || msg_block <= head.finalized
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FinalityError {
    #[error("invalid source RPC URL: {0}")]
    InvalidUrl(String),
    #[error("source RPC request failed: {0}")]
    Rpc(String),
    #[error("source chain returned no {0} block")]
    MissingBlock(&'static str),
}

/// Reads source-chain head positions. Abstracted for testability.
#[async_trait]
pub trait SourceFinalityReader: Send + Sync {
    async fn source_head(&self) -> Result<SourceHead, FinalityError>;
}

/// HTTP-RPC-backed reader with a short head cache.
pub struct AlloyFinalityReader {
    provider: RootProvider<Http<Client>>,
    cache: Mutex<Option<(SourceHead, Instant)>>,
}

impl AlloyFinalityReader {
    /// Build a reader against `rpc_url`.
    pub fn new(rpc_url: &str) -> Result<Self, FinalityError> {
        let url = rpc_url
            .parse()
            .map_err(|e| FinalityError::InvalidUrl(format!("{rpc_url}: {e}")))?;
        Ok(Self {
            provider: RootProvider::new_http(url),
            cache: Mutex::new(None),
        })
    }

    async fn block_number_for(
        &self,
        tag: BlockNumberOrTag,
        label: &'static str,
    ) -> Result<u64, FinalityError> {
        let block = self
            .provider
            .get_block_by_number(tag, BlockTransactionsKind::Hashes)
            .await
            .map_err(|e| FinalityError::Rpc(e.to_string()))?
            .ok_or(FinalityError::MissingBlock(label))?;
        Ok(block.header.number)
    }
}

#[async_trait]
impl SourceFinalityReader for AlloyFinalityReader {
    async fn source_head(&self) -> Result<SourceHead, FinalityError> {
        let cached = self.cache.lock().await.as_ref().copied();
        match cached {
            Some((head, fetched_at)) if fetched_at.elapsed() < HEAD_CACHE_TTL => {
                return Ok(head);
            }
            _ => {}
        }

        let latest = self
            .provider
            .get_block_number()
            .await
            .map_err(|e| FinalityError::Rpc(e.to_string()))?;
        let finalized = self
            .block_number_for(BlockNumberOrTag::Finalized, "finalized")
            .await?;
        let safe = self
            .block_number_for(BlockNumberOrTag::Safe, "safe")
            .await?;

        let head = SourceHead {
            latest,
            finalized,
            safe,
        };
        *self.cache.lock().await = Some((head, Instant::now()));
        Ok(head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_finality_default_is_finalized() {
        assert_eq!(parse_finality(0x0000_0000), FinalityRequirement::Finalized);
    }

    #[test]
    fn parse_finality_safe_flag() {
        assert_eq!(parse_finality(0x0001_0000), FinalityRequirement::Safe);
        // Safe flag wins even if depth bits are set.
        assert_eq!(parse_finality(0x0001_000A), FinalityRequirement::Safe);
    }

    #[test]
    fn parse_finality_confirmation_depth() {
        assert_eq!(parse_finality(1), FinalityRequirement::Confirmations(1));
        assert_eq!(
            parse_finality(0x0000_FFFF),
            FinalityRequirement::Confirmations(0xFFFF)
        );
    }

    fn head(latest: u64, finalized: u64, safe: u64) -> SourceHead {
        SourceHead {
            latest,
            finalized,
            safe,
        }
    }

    #[test]
    fn finalized_ready_only_when_block_is_finalized() {
        let h = head(100, 90, 95);
        assert!(is_ready(&FinalityRequirement::Finalized, 90, &h));
        assert!(!is_ready(&FinalityRequirement::Finalized, 91, &h));
    }

    #[test]
    fn safe_ready_only_when_block_is_safe() {
        let h = head(100, 90, 95);
        assert!(is_ready(&FinalityRequirement::Safe, 95, &h));
        assert!(!is_ready(&FinalityRequirement::Safe, 96, &h));
    }

    #[test]
    fn confirmations_ready_by_depth_or_finalization() {
        let h = head(100, 90, 95);
        // depth: block + 5 <= latest(100) -> block <= 95
        assert!(is_ready(&FinalityRequirement::Confirmations(5), 95, &h));
        assert!(!is_ready(&FinalityRequirement::Confirmations(5), 96, &h));
        // OR cap at finalization: a huge depth still passes if finalized.
        assert!(is_ready(
            &FinalityRequirement::Confirmations(10_000),
            90,
            &h
        ));
        assert!(!is_ready(
            &FinalityRequirement::Confirmations(10_000),
            91,
            &h
        ));
    }
}
