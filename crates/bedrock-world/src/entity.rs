//! Bedrock actor/entity records, actor-index identities and actor-storage migration.

/// Migration between legacy inline actor storage and modern digest/payload storage.
pub mod migration;

pub use crate::chunk::key::{ActorDigestKey, ActorUid};
pub use crate::chunk::EntityData;
pub use crate::parsed::{
    ActorRecord, ActorResolution, ActorSource, ParsedActorDigest, ParsedEntity,
    encode_actor_digest_ids, parse_actor_digest_ids,
};
pub use migration::{
    ActorMigrationAction, ActorMigrationReport, actor_storage_compatibility,
    classify_actor_migration, migrate_inline_actor_chunk_blocking,
};
