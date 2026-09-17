use thiserror::Error;

#[derive(Debug, Error)]
pub enum FaError {
    #[error("bridge: {0}")]
    Bridge(#[from] fa_bridge::BridgeError),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("backend: {0}")]
    Backend(String),
    #[error("tool protocol: {0}")]
    Protocol(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("approval denied by operator")]
    Denied,
    #[error("tool {tool} failed: {message}")]
    ToolFailed { tool: String, message: String },
    #[error("rpc: {0}")]
    Rpc(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, FaError>;
