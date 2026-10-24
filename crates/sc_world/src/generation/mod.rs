//! 内置三层世界生成器的身份与状态契约。
//!
//! 包含生成器描述符、特征身份、读写范围、区块生成记录与契约错误。
//!
//! Identity and state contracts for the built-in three-layer world generator.
//!
//! Includes the generator descriptor, feature identities, extent bounds, chunk generation records, and contract errors.

pub mod assemble;
pub mod descriptor;
pub mod error;
pub mod extent;
pub mod identity;
pub mod owner;
pub mod record;
pub mod tracker;
pub mod view;

pub use assemble::assemble_final_candidate;
pub use descriptor::GeneratorDescriptor;
pub use error::GenerationContractError;
pub use extent::{ExtentRegistry, FeatureExtent};
pub use identity::{FeatureId, FeatureType, FragmentId, GenerationStage, OriginCell};
pub use owner::{
    ChunkFragmentPayload, FinalCandidatePayload, FragmentBlockWrite, OwnerEpoch,
    RegionOwner, RegionOwnerKey, RegionPublishEvent, RegionSubmissionMailbox,
    SubmissionTicket, WorkerSubmission, DEFAULT_REGION_SIZE_CHUNKS,
};
pub use record::{
    ChunkGenerationRecord, GenerationControlState, PersistedGenerationRecord, ReadyCertificate,
    SealBookkeeping,
};
pub use tracker::TargetChunkTracker;
pub use view::{BaseSnapshot, GenerationView};
