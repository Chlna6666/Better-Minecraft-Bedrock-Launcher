//! Minecraft Bedrock actor/entity records and persisted actor-storage representations.

/// Explicit conversion between inline actor records and digest/payload actor storage.
pub mod conversion;

pub use crate::chunk::key::{ActorDigestKey, ActorUid};
pub use crate::chunk::EntityData;
pub use crate::parsed::{
    ActorRecord, ActorResolution, ActorSource, ParsedActorDigest, ParsedEntity,
    encode_actor_digest_ids, parse_actor_digest_ids,
};
pub use conversion::{
    ActorStorageConversion, ActorStorageConversionReport, ActorStorageTarget,
    actor_storage_compatibility, actor_storage_conversion_compatibility,
    classify_actor_storage_conversion, convert_digest_actor_chunk_to_inline_blocking,
    convert_inline_actor_chunk_to_digest_blocking,
};
