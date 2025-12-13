use crate::show_windows_error;
use thiserror::Error;
use tokio::task::JoinError;
use zip::result::ZipError;

/// 核心错误类型
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XML parsing error: {0}")]
    Xml(#[from] xmltree::ParseError),

    #[error("Zip error: {0}")]
    Zip(#[from] ZipError),

    #[error("Bad update identity")]
    BadUpdateIdentity,

    #[error("Unknown content length")]
    UnknownContentLength,

    #[error("Task join error: {0}")]
    Join(#[from] JoinError),

    #[error("Config error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),

    #[error("Operation timed out")]
    Timeout,

    /// 校验和不匹配（例如 MD5 校验失败）
    #[error("Checksum mismatch: {0}")]
    ChecksumMismatch(String),
}

impl From<tokio::time::error::Elapsed> for CoreError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        CoreError::Timeout
    }
}

/// 核心结果类型
#[derive(Debug)]
pub enum CoreResult<T = ()> {
    Success(T),
    Cancelled,
    Error(CoreError),
}

impl<T> CoreResult<T> {
    pub fn success(value: T) -> Self {
        CoreResult::Success(value)
    }

    pub fn cancelled() -> Self {
        CoreResult::Cancelled
    }

    pub fn error(err: CoreError) -> Self {
        CoreResult::Error(err)
    }
}

impl<T> From<Result<T, CoreError>> for CoreResult<T> {
    fn from(r: Result<T, CoreError>) -> Self {
        match r {
            Ok(v) => CoreResult::Success(v),
            Err(e) => {
                let msg = e.to_string();
                // 如果当前进程/线程没有 tokio 运行时，这里会 panic！
                // 因此使用前请非常确定应用一定运行在 tokio runtime 中。
                let _ = tokio::task::spawn_blocking(move || {
                    show_windows_error("程序错误", &msg);
                });
                CoreResult::Error(e)
            }
        }
    }
}
