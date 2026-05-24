pub mod noop;
pub mod resume;
pub mod shadow_git;
pub mod store;
pub mod types;

pub use noop::NoopStore;
pub use resume::{compute_resume_plan, ResumePlan};
pub use shadow_git::ShadowGitStore;
pub use store::{CheckpointStore, SwitchingCheckpointStore};
pub use types::{
    CheckpointDiff, CheckpointError, CheckpointId, CheckpointKind, CheckpointMeta,
    CheckpointRecordRequest, CheckpointRestoreReport, ListOptions, RestoreOptions, RetentionPolicy,
};

#[cfg(test)]
mod tests;
