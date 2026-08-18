//! Minecraft Bedrock actor/entity records and persisted actor-storage representations.

/// Explicit conversion between historical inline actor storage and digest/payload storage.
pub mod conversion;

pub use crate::chunk::key::{ActorDigestKey, ActorUid};
pub use crate::chunk::EntityData;
pub use crate::parsed::{
    ActorRecord, ActorResolution, ActorSource, ParsedActorDigest, ParsedEntity,
    encode_actor_digest_ids, parse_actor_digest_ids,
};
pub use conversion::{
    ActorMigrationAction, ActorMigrationReport, actor_storage_compatibility,
    classify_actor_migration, migrate_inline_actor_chunk_blocking,
};
