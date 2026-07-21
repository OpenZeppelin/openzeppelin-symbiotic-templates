//! Finalized source-chain reconciliation sweep.

use std::sync::Arc;
use std::time::Duration;

use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider as AlloyProvider, RootProvider};
use alloy::rpc::types::{BlockTransactionsKind, Filter, Log};
use alloy::transports::http::{Client, Http};
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::config::SweepSettings;
use crate::error::{ProviderError, StorageError};
use crate::provider::{
    DynProvider, IngestionContext, IngestionOrigin, IngestionOutcome, SweepFilter,
};
use crate::storage::Storage;

const MIN_BLOCK_RANGE: u64 = 1;
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum SweepError {
    #[error("invalid source RPC URL: {0}")]
    InvalidUrl(String),
    #[error("source RPC request failed: {0}")]
    Rpc(String),
    #[error("source RPC request timed out")]
    Timeout,
    #[error("source chain returned no finalized block")]
    MissingFinalizedBlock,
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("provider ingestion error: {0}")]
    Ingestion(#[from] ProviderError),
}

impl SweepError {
    fn is_range_limit(&self) -> bool {
        let Self::Rpc(message) = self else {
            return false;
        };
        let message = message.to_ascii_lowercase();
        message.contains("block range")
            || message.contains("too many results")
            || message.contains("query returned more")
            || message.contains("response size")
            || message.contains("result size")
            || message.contains("-32005")
            || message.contains("limit exceeded")
            || message.contains("range too large")
            || message.contains("exceeds")
            || message.contains("10000")
    }
}

#[async_trait]
pub trait SweepRpc: Send + Sync {
    async fn finalized_head(&self) -> Result<u64, SweepError>;

    async fn get_logs(
        &self,
        filter: SweepFilter,
        from: u64,
        to: u64,
    ) -> Result<Vec<Log>, SweepError>;
}

pub struct AlloySweepRpc {
    provider: RootProvider<Http<Client>>,
}

impl AlloySweepRpc {
    #[allow(clippy::result_large_err)]
    pub fn new(rpc_url: &str) -> Result<Self, SweepError> {
        let url = rpc_url
            .parse()
            .map_err(|error| SweepError::InvalidUrl(format!("{rpc_url}: {error}")))?;
        Ok(Self {
            provider: RootProvider::new_http(url),
        })
    }
}

#[async_trait]
impl SweepRpc for AlloySweepRpc {
    async fn finalized_head(&self) -> Result<u64, SweepError> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Finalized, BlockTransactionsKind::Hashes)
            .await
            .map_err(|error| SweepError::Rpc(error.to_string()))?
            .ok_or(SweepError::MissingFinalizedBlock)?;
        Ok(block.header.number)
    }

    async fn get_logs(
        &self,
        sweep_filter: SweepFilter,
        from: u64,
        to: u64,
    ) -> Result<Vec<Log>, SweepError> {
        let filter = Filter::new()
            .address(sweep_filter.address)
            .event_signature(sweep_filter.topic0)
            .from_block(from)
            .to_block(to);
        self.provider
            .get_logs(&filter)
            .await
            .map_err(|error| SweepError::Rpc(error.to_string()))
    }
}

pub struct SweepJob {
    storage: Arc<Storage>,
    provider: DynProvider,
    rpc: Arc<dyn SweepRpc>,
    source_chain_id: u64,
    settings: SweepSettings,
}

impl SweepJob {
    pub fn new(
        storage: Arc<Storage>,
        provider: DynProvider,
        rpc: Arc<dyn SweepRpc>,
        source_chain_id: u64,
        settings: SweepSettings,
    ) -> Self {
        Self {
            storage,
            provider,
            rpc,
            source_chain_id,
            settings,
        }
    }

    pub async fn run(self, mut shutdown_rx: broadcast::Receiver<()>) -> Result<(), SweepError> {
        let mut interval = tokio::time::interval(Duration::from_secs(self.settings.interval_secs));
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!(target: "operator::sweep", "reconciliation sweep shutting down");
                    return Ok(());
                }
                _ = interval.tick() => {
                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            tracing::info!(target: "operator::sweep", "reconciliation sweep shutting down");
                            return Ok(());
                        }
                        result = self.process_tick() => {
                            if let Err(error) = result {
                                tracing::warn!(target: "operator::sweep", error = %error, "reconciliation sweep tick failed; cursor unchanged for failed range");
                            }
                        }
                    }
                }
            }
        }
    }

    async fn process_tick(&self) -> Result<(), SweepError> {
        let finalized = tokio::time::timeout(RPC_TIMEOUT, self.rpc.finalized_head())
            .await
            .map_err(|_| SweepError::Timeout)??;

        for filter in self.provider.sweep_filters() {
            self.process_filter(filter, finalized).await?;
        }
        Ok(())
    }

    async fn process_filter(&self, filter: SweepFilter, finalized: u64) -> Result<(), SweepError> {
        let mut cursor = match self.storage.get_sweep_cursor(
            self.source_chain_id,
            &filter.address,
            &filter.topic0,
        )? {
            Some(cursor) => cursor,
            None => {
                let configured_cursor = self.settings.start_block.unwrap_or(finalized);
                let oldest_pending_block = self.storage.oldest_noncanonical_pending_block()?;
                let cursor = oldest_pending_block
                    .map(|block| configured_cursor.min(block))
                    .unwrap_or(configured_cursor);
                let reason = if oldest_pending_block.is_some_and(|block| block < configured_cursor)
                {
                    "oldest non-canonical pending message"
                } else if self.settings.start_block.is_some() {
                    "configured startBlock"
                } else {
                    "finalized head"
                };
                self.storage.set_sweep_cursor(
                    self.source_chain_id,
                    &filter.address,
                    &filter.topic0,
                    cursor,
                )?;
                tracing::info!(
                    target: "operator::sweep",
                    cursor,
                    finalized,
                    start_block = ?self.settings.start_block,
                    oldest_pending_block,
                    reason,
                    address = %filter.address,
                    topic0 = %filter.topic0,
                    "initialized reconciliation sweep cursor"
                );
                if self.settings.start_block.is_none() && oldest_pending_block.is_none() {
                    tracing::warn!(
                        target: "operator::sweep",
                        cursor,
                        address = %filter.address,
                        topic0 = %filter.topic0,
                        "no sweep startBlock configured; history before finalized head will not be swept"
                    );
                }
                cursor
            }
        };
        let mut block_range = self.settings.max_block_range;

        while cursor <= finalized {
            let range_end = cursor
                .saturating_add(block_range.saturating_sub(1))
                .min(finalized);
            let logs = match tokio::time::timeout(
                RPC_TIMEOUT,
                self.rpc.get_logs(filter, cursor, range_end),
            )
            .await
            {
                Err(_) => return Err(SweepError::Timeout),
                Ok(Err(error)) if error.is_range_limit() => {
                    let attempted_span = range_end.saturating_sub(cursor).saturating_add(1);
                    if attempted_span == MIN_BLOCK_RANGE {
                        return Err(error);
                    }
                    block_range = (attempted_span / 2).max(MIN_BLOCK_RANGE);
                    tracing::warn!(
                        target: "operator::sweep",
                        from = cursor,
                        to = range_end,
                        retry_block_range = block_range,
                        error = %error,
                        "source RPC rejected sweep range; retrying a smaller range"
                    );
                    continue;
                }
                Ok(Err(error)) => return Err(error),
                Ok(Ok(logs)) => logs,
            };

            let logs_found = logs.len();
            let mut inserted = 0usize;
            let mut duplicates = 0usize;
            let mut conflicts = 0usize;
            let ctx = IngestionContext {
                origin: IngestionOrigin::Sweep,
                source_chain_id: self.source_chain_id,
            };
            for log in &logs {
                let outcome = match self.provider.ingest_log(log, &ctx) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        tracing::error!(
                            target: "operator::sweep",
                            from = cursor,
                            to = range_end,
                            block = ?log.block_number,
                            transaction_hash = ?log.transaction_hash,
                            error = %error,
                            "filtered source log failed to decode; sweep cursor pinned"
                        );
                        return Err(SweepError::Ingestion(error));
                    }
                };
                match outcome {
                    IngestionOutcome::Inserted => {
                        inserted += 1;
                        tracing::warn!(
                            target: "operator::sweep",
                            block = ?log.block_number,
                            transaction_hash = ?log.transaction_hash,
                            "recovered source message missed by webhook delivery"
                        );
                    }
                    IngestionOutcome::ExactDuplicate => duplicates += 1,
                    IngestionOutcome::Conflict => conflicts += 1,
                    IngestionOutcome::Irrelevant => {}
                }
            }

            let Some(next_cursor) = range_end.checked_add(1) else {
                self.storage.set_sweep_cursor(
                    self.source_chain_id,
                    &filter.address,
                    &filter.topic0,
                    u64::MAX,
                )?;
                tracing::info!(
                    target: "operator::sweep",
                    from = cursor,
                    to = range_end,
                    logs_found,
                    inserted,
                    duplicates,
                    conflicts,
                    cursor = u64::MAX,
                    "completed finalized reconciliation range at maximum block; stopping catch-up"
                );
                break;
            };
            self.storage.set_sweep_cursor(
                self.source_chain_id,
                &filter.address,
                &filter.topic0,
                next_cursor,
            )?;
            tracing::info!(
                target: "operator::sweep",
                from = cursor,
                to = range_end,
                logs_found,
                inserted,
                duplicates,
                conflicts,
                cursor = next_cursor,
                "completed finalized reconciliation range"
            );
            cursor = next_cursor;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use alloy::primitives::{Address, B256, Bytes, LogData};
    use axum::Router;

    use super::*;
    use crate::api::AppState;
    use crate::provider::Provider;
    use crate::storage::{MessageData, MessageMetadata};
    use crate::webhook::WebhookEvent;
    use tempfile::tempdir;

    const CHAIN_ID: u64 = 31_337;

    fn filter() -> SweepFilter {
        SweepFilter {
            address: Address::from_slice(&[0x11u8; 20]),
            topic0: B256::from_slice(&[0x22u8; 32]),
        }
    }

    struct TestProvider {
        fail_ingest: bool,
    }

    #[async_trait]
    impl Provider for TestProvider {
        fn name(&self) -> &'static str {
            "test"
        }

        async fn handle_webhook_event(&self, _event: &WebhookEvent) -> Result<(), ProviderError> {
            Ok(())
        }

        fn register_api_routes(&self, router: Router<AppState>) -> Router<AppState> {
            router
        }

        fn sweep_filters(&self) -> Vec<SweepFilter> {
            vec![filter()]
        }

        fn ingest_log(
            &self,
            _log: &Log,
            _ctx: &IngestionContext,
        ) -> Result<IngestionOutcome, ProviderError> {
            if self.fail_ingest {
                Err(ProviderError::EventDecode(
                    "mock contract upgrade".to_string(),
                ))
            } else {
                Ok(IngestionOutcome::ExactDuplicate)
            }
        }
    }

    struct MockRpc {
        finalized: u64,
        fail_get_logs: bool,
        return_log: bool,
        max_accepted_range: Option<u64>,
        calls: StdMutex<Vec<(u64, u64)>>,
    }

    impl MockRpc {
        fn new(finalized: u64) -> Self {
            Self {
                finalized,
                fail_get_logs: false,
                return_log: false,
                max_accepted_range: None,
                calls: StdMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SweepRpc for MockRpc {
        async fn finalized_head(&self) -> Result<u64, SweepError> {
            Ok(self.finalized)
        }

        async fn get_logs(
            &self,
            sweep_filter: SweepFilter,
            from: u64,
            to: u64,
        ) -> Result<Vec<Log>, SweepError> {
            self.calls.lock().unwrap().push((from, to));
            if self.fail_get_logs {
                return Err(SweepError::Rpc("mock RPC unavailable".to_string()));
            }
            if self
                .max_accepted_range
                .is_some_and(|max| to.saturating_sub(from).saturating_add(1) > max)
            {
                return Err(SweepError::Rpc("-32005: limit exceeded".to_string()));
            }
            if !self.return_log {
                return Ok(Vec::new());
            }
            Ok(vec![Log {
                inner: alloy::primitives::Log {
                    address: sweep_filter.address,
                    data: LogData::new_unchecked(vec![sweep_filter.topic0], Bytes::new()),
                },
                block_hash: None,
                block_number: Some(from),
                block_timestamp: None,
                transaction_hash: Some(B256::from_slice(&[0x33u8; 32])),
                transaction_index: None,
                log_index: Some(0),
                removed: false,
            }])
        }
    }

    fn job(
        rpc: Arc<MockRpc>,
        storage: Arc<Storage>,
        settings: SweepSettings,
        fail_ingest: bool,
    ) -> SweepJob {
        let provider: DynProvider = Arc::new(TestProvider { fail_ingest });
        SweepJob::new(storage, provider, rpc, CHAIN_ID, settings)
    }

    fn storage() -> (Arc<Storage>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let storage = Storage::new_with_provider(dir.path().join("test.db"), "test").unwrap();
        (Arc::new(storage), dir)
    }

    fn seed_pending_message(storage: &Storage, block_number: u64) {
        let message_id = B256::from_slice(&[0x55u8; 32]);
        storage
            .save_message(&MessageData {
                metadata: MessageMetadata {
                    source_chain: CHAIN_ID,
                    destination_chain: 31_338,
                    block_number,
                    message_id,
                    event_tx_hash: B256::from_slice(&[0x66u8; 32]),
                    ttl: None,
                },
                data: vec![0x77],
            })
            .unwrap();
    }

    #[tokio::test]
    async fn initializes_cursor_from_configured_start_block() {
        let (storage, _dir) = storage();
        let rpc = Arc::new(MockRpc::new(50));
        let settings = SweepSettings {
            start_block: Some(100),
            ..SweepSettings::default()
        };

        job(rpc, storage.clone(), settings, false)
            .process_tick()
            .await
            .unwrap();

        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(100)
        );
    }

    #[tokio::test]
    async fn initializes_cursor_from_oldest_noncanonical_pending_message() {
        let (storage, _dir) = storage();
        seed_pending_message(&storage, 100);
        let mut rpc = MockRpc::new(500);
        rpc.fail_get_logs = true;
        let rpc = Arc::new(rpc);

        assert!(
            job(rpc, storage.clone(), SweepSettings::default(), false)
                .process_tick()
                .await
                .is_err()
        );

        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(100)
        );
    }

    #[tokio::test]
    async fn configured_start_block_precedes_oldest_pending_message() {
        let (storage, _dir) = storage();
        seed_pending_message(&storage, 100);
        let mut rpc = MockRpc::new(500);
        rpc.fail_get_logs = true;
        let rpc = Arc::new(rpc);
        let settings = SweepSettings {
            start_block: Some(50),
            ..SweepSettings::default()
        };

        assert!(
            job(rpc, storage.clone(), settings, false)
                .process_tick()
                .await
                .is_err()
        );

        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(50)
        );
    }

    #[tokio::test]
    async fn initializes_cursor_from_finalized_head_without_pending_messages() {
        let (storage, _dir) = storage();
        let mut rpc = MockRpc::new(500);
        rpc.fail_get_logs = true;
        let rpc = Arc::new(rpc);

        assert!(
            job(rpc, storage.clone(), SweepSettings::default(), false)
                .process_tick()
                .await
                .is_err()
        );

        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(500)
        );
    }

    #[tokio::test]
    async fn empty_successful_range_advances_cursor_after_processing() {
        let (storage, _dir) = storage();
        let rpc = Arc::new(MockRpc::new(5));
        let settings = SweepSettings {
            start_block: Some(5),
            ..SweepSettings::default()
        };

        job(rpc.clone(), storage.clone(), settings, false)
            .process_tick()
            .await
            .unwrap();

        assert_eq!(*rpc.calls.lock().unwrap(), vec![(5, 5)]);
        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(6)
        );
    }

    #[tokio::test]
    async fn rpc_error_does_not_advance_cursor() {
        let (storage, _dir) = storage();
        let mut rpc = MockRpc::new(20);
        rpc.fail_get_logs = true;
        let rpc = Arc::new(rpc);
        let settings = SweepSettings {
            start_block: Some(10),
            ..SweepSettings::default()
        };

        assert!(
            job(rpc, storage.clone(), settings, false)
                .process_tick()
                .await
                .is_err()
        );

        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(10)
        );
    }

    #[tokio::test]
    async fn filtered_decode_error_pins_cursor() {
        let (storage, _dir) = storage();
        let mut rpc = MockRpc::new(10);
        rpc.return_log = true;
        let rpc = Arc::new(rpc);
        let settings = SweepSettings {
            start_block: Some(10),
            ..SweepSettings::default()
        };

        assert!(
            job(rpc, storage.clone(), settings, true)
                .process_tick()
                .await
                .is_err()
        );

        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(10)
        );
    }

    #[tokio::test]
    async fn chunks_wide_span_using_inclusive_bounds() {
        let (storage, _dir) = storage();
        let rpc = Arc::new(MockRpc::new(2_500));
        let settings = SweepSettings {
            max_block_range: 1_000,
            start_block: Some(1),
            ..SweepSettings::default()
        };

        job(rpc.clone(), storage.clone(), settings, false)
            .process_tick()
            .await
            .unwrap();

        assert_eq!(
            *rpc.calls.lock().unwrap(),
            vec![(1, 1_000), (1_001, 2_000), (2_001, 2_500)]
        );
        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(2_501)
        );
    }

    #[tokio::test]
    async fn range_limit_halves_and_retries_without_advancing() {
        let (storage, _dir) = storage();
        let mut rpc = MockRpc::new(8);
        rpc.max_accepted_range = Some(1);
        let rpc = Arc::new(rpc);
        let settings = SweepSettings {
            max_block_range: 100,
            start_block: Some(1),
            ..SweepSettings::default()
        };

        job(rpc.clone(), storage.clone(), settings, false)
            .process_tick()
            .await
            .unwrap();

        let calls = rpc.calls.lock().unwrap();
        assert!(calls.starts_with(&[(1, 8), (1, 4), (1, 2), (1, 1)]));
        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(9)
        );
    }

    #[test]
    fn alloy_sweep_rpc_rejects_malformed_url() {
        assert!(matches!(
            AlloySweepRpc::new("not-a-url"),
            Err(SweepError::InvalidUrl(_))
        ));
    }

    #[test]
    fn recognizes_common_range_limit_errors() {
        for message in [
            "-32005",
            "limit exceeded",
            "range too large",
            "requested range exceeds provider limit",
            "query is limited to 10000 blocks",
        ] {
            assert!(SweepError::Rpc(message.to_string()).is_range_limit());
        }
    }

    #[tokio::test]
    async fn single_block_range_limit_error_is_returned() {
        let (storage, _dir) = storage();
        let mut rpc = MockRpc::new(1);
        rpc.max_accepted_range = Some(0);
        let rpc = Arc::new(rpc);
        let settings = SweepSettings {
            max_block_range: 100,
            start_block: Some(1),
            ..SweepSettings::default()
        };

        assert!(
            job(rpc.clone(), storage.clone(), settings, false)
                .process_tick()
                .await
                .is_err()
        );
        assert_eq!(*rpc.calls.lock().unwrap(), vec![(1, 1)]);
        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(1)
        );
    }

    #[tokio::test]
    async fn maximum_block_terminates_catch_up() {
        let (storage, _dir) = storage();
        let rpc = Arc::new(MockRpc::new(u64::MAX));
        let settings = SweepSettings {
            start_block: Some(u64::MAX),
            ..SweepSettings::default()
        };

        tokio::time::timeout(
            Duration::from_secs(1),
            job(rpc.clone(), storage.clone(), settings, false).process_tick(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(*rpc.calls.lock().unwrap(), vec![(u64::MAX, u64::MAX)]);
        let filter = filter();
        assert_eq!(
            storage
                .get_sweep_cursor(CHAIN_ID, &filter.address, &filter.topic0)
                .unwrap(),
            Some(u64::MAX)
        );
    }
}
