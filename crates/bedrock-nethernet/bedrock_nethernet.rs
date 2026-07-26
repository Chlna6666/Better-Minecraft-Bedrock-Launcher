mod discovery;
mod session;
mod transport;

pub use discovery::{DiscoveredServer, LanSignaling, ServerData};
pub use session::NethernetSession;
pub use transport::{NethernetListener, NethernetStream};

pub type Result<T> = std::result::Result<T, NethernetError>;

#[derive(Debug, thiserror::Error)]
pub enum NethernetError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("WebRTC error: {0}")]
    WebRtc(#[from] webrtc::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("connection timed out")]
    Timeout,
    #[error("connection closed")]
    Closed,
}
