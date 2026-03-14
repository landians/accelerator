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

impl CacheError {
    /// Returns a stable error kind tag for metrics/logging aggregation.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidConfig(_) => "invalid_config",
            Self::Backend(_) => "backend",
            Self::Loader(_) => "loader",
            Self::Timeout(_) => "timeout",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CacheError;

    #[test]
    fn cache_error_kind_returns_stable_tags() {
        assert_eq!(
            CacheError::InvalidConfig("x".to_string()).kind(),
            "invalid_config"
        );
        assert_eq!(CacheError::Backend("x".to_string()).kind(), "backend");
        assert_eq!(CacheError::Loader("x".to_string()).kind(), "loader");
        assert_eq!(CacheError::Timeout("loader").kind(), "timeout");
    }
}
