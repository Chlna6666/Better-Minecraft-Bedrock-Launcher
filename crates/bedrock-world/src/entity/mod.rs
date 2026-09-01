//! Minecraft Bedrock `Entity`, `digp` and `actorprefix` records.

mod actor_storage;
mod digp;
mod ownership;

pub use crate::chunk::EntityData;
pub use crate::chunk::key::{ActorDigestKey, ActorUid};
pub use crate::scan::{
    ActorRecord, ActorResolution, ActorSource, ActorDigest, Actor,
    encode_actor_ids, decode_actor_ids,
};
pub use actor_storage::{ActorStorageRewriteReport, ActorUidRepairReport};
pub(crate) use actor_storage::{
    stage_actor_uid_repair, stage_world_digp_actorprefix_to_entity,
    stage_world_entity_to_digp_actorprefix,
};
pub use digp::{ActorRecordWriteReport, write_digp_from_entity, write_entity_from_digp};
pub use ownership::ActorOwnershipIndex;
