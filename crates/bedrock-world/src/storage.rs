//! Storage abstraction used by `bedrock-world`.
//!
//! World-level raw record access is separated from Minecraft semantics and Mojang LevelDB internals.
//! Public consumers use this module instead of depending on `bedrock-leveldb` details directly.

mod implementation {
    // The large storage implementation is being split mechanically. During that split, keep its
    // existing `bedrock_leveldb::Type` spelling local to this private module while resolving every
    // symbol from the 0.7 responsibility modules. This is not a public compatibility API.
    #[cfg(feature = "backend-bedrock-leveldb")]
    mod leveldb_07 {
        pub use ::bedrock_leveldb::access::*;
        pub use ::bedrock_leveldb::engine::*;
        pub use ::bedrock_leveldb::error::*;
        pub use ::bedrock_leveldb::format::*;
    }
    #[cfg(feature = "backend-bedrock-leveldb")]
    use leveldb_07 as bedrock_leveldb;

    include!("storage/impl.rs");
}

pub mod adapters;
pub mod core;
pub mod memory;
pub mod pipeline;

pub use implementation::*;
