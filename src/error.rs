use thiserror::Error;

pub type CacheResult<T> = Result<T, CacheError>;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CacheError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("loader error: {0}")]
    Loader(String),
    #[error("timeout while executing {0}")]
    Timeout(&'static str),
}
