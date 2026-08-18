//! Minecraft Bedrock `Entity`, `digp` and `actorprefix` records.

mod actor_storage;
mod digp;

pub use crate::chunk::EntityData;
pub use crate::chunk::key::{ActorDigestKey, ActorUid};
pub use crate::parsed::{
    ActorRecord, ActorResolution, ActorSource, ParsedActorDigest, ParsedEntity,
    encode_actor_digest_ids, parse_actor_digest_ids,
};
pub use actor_storage::ActorStorageRewriteReport;
pub(crate) use actor_storage::{
    stage_world_digp_actorprefix_to_entity, stage_world_entity_to_digp_actorprefix,
};
pub use digp::{ActorRecordWriteReport, write_digp_from_entity, write_entity_from_digp};
