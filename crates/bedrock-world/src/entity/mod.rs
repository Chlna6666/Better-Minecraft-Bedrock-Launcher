//! Minecraft Bedrock `Entity`, `digp` and `actorprefix` records.

mod actor_storage;
mod digp;
mod ownership;

pub use crate::chunk::EntityData;
pub use crate::chunk::key::{ActorDigestKey, ActorUid};
pub use crate::parsed::{
    ActorRecord, ActorResolution, ActorSource, ParsedActorDigest, ParsedEntity,
    encode_actor_digest_ids, parse_actor_digest_ids,
};
pub use actor_storage::{ActorStorageRewriteReport, ActorUidRepairReport};
pub(crate) use actor_storage::{
    stage_actor_uid_repair, stage_world_digp_actorprefix_to_entity,
    stage_world_entity_to_digp_actorprefix,
};
pub use digp::{ActorRecordWriteReport, write_digp_from_entity, write_entity_from_digp};
pub use ownership::ActorOwnershipIndex;
