use thiserror::Error;

/// Provider-related errors
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("unknown event type: {0}")]
    UnknownEvent(String),

    #[error("unknown LayerZero endpoint ID: {0}")]
    UnknownEid(u32),

    #[error("missing transaction in webhook event")]
    MissingTransaction,

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("event decoding error: {0}")]
    EventDecode(String),
}

/// Symbiotic relay client errors (for BLS signature aggregation)
#[derive(Error, Debug)]
pub enum SymbioticRelayError {
    #[error("connection failed: {0}")]
    Connection(#[from] tonic::transport::Error),

    #[error("rpc error: {0}")]
    Rpc(#[from] tonic::Status),

    #[error("proof not ready")]
    NotReady,

    #[error("invalid address: {0}")]
    InvalidAddress(String),
}

/// Storage errors
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] redb::Error),

    #[error("database error: {0}")]
    DatabaseErr(#[from] redb::DatabaseError),

    #[error("commit error: {0}")]
    Commit(#[from] redb::CommitError),

    #[error("transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),

    #[error("table error: {0}")]
    Table(#[from] redb::TableError),

    #[error("storage error: {0}")]
    Storage(#[from] redb::StorageError),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("key not found: {0}")]
    NotFound(String),

    #[error("key already exists")]
    KeyExists,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Signer job errors
#[derive(Error, Debug)]
pub enum SignerError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("symbiotic relay error: {0}")]
    SymbioticRelay(#[from] SymbioticRelayError),

    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("merkle tree not found")]
    TreeNotFound,

    #[error("empty merkle tree")]
    EmptyTree,

    #[error("evm client error: {0}")]
    EvmClient(String),

    #[error("proof not ready, will retry")]
    ProofNotReady,
}

/// OpenZeppelin Relayer client errors
#[derive(Error, Debug)]
pub enum RelayerError {
    #[error("http client error: {0}")]
    HttpClient(String),

    #[error("http request error: {0}")]
    HttpRequest(String),

    #[error("API error (status {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("chain not configured: {0}")]
    ChainNotConfigured(u64),

    #[error("transaction not found: {0}")]
    TransactionNotFound(String),

    #[error("message not found: {0}")]
    MessageNotFound(alloy::primitives::B256),

    #[error("proof generation failed: {0}")]
    ProofGeneration(String),

    #[error("epoch missing from merkle tree")]
    EpochMissing,

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("deserialization error: {0}")]
    Deserialization(#[from] serde_json::Error),
}

/// API errors
#[derive(Error, Debug)]
pub enum ApiError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),
}

/// Security middleware errors
#[derive(Error, Debug)]
pub enum SecurityError {
    #[error("missing signature header")]
    MissingSignature,

    #[error("missing timestamp header")]
    MissingTimestamp,

    #[error("invalid timestamp format")]
    InvalidTimestamp,

    #[error("timestamp expired")]
    TimestampExpired,

    #[error("invalid signature")]
    InvalidSignature,

    #[error("invalid secret")]
    InvalidSecret,
}

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("config error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<ApiError> for axum::http::StatusCode {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::NotFound(_) => axum::http::StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => axum::http::StatusCode::BAD_REQUEST,
            ApiError::Provider(_) | ApiError::Storage(_) | ApiError::Internal(_) => {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}
