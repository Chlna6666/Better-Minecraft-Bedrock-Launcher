//! 协议层：线格式编解码，不含任何 IO。

pub(crate) mod codec;
pub(crate) mod crypto;
pub mod message;
pub mod packet;
pub mod server_data;
pub mod signal;

pub use message::{Reassembler, split};
pub use packet::DiscoveryPacket;
pub use server_data::{ServerData, game_type, transport_layer};
pub use signal::{Signal, SignalType};
