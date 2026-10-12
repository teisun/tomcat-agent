//! Runtime cache for the plan file currently associated with a session.
//!
//! The file remains authoritative. This value only keeps its identity, resolved path and
//! last-read lifecycle state together so consumers cannot accidentally combine fields from
//! different plans.

use std::path::PathBuf;

use super::file_store::{PlanFile, PlanFileState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePlan {
    pub id: String,
    pub path: PathBuf,
    pub state: PlanFileState,
}

impl ActivePlan {
    pub fn from_file(path: PathBuf, plan: &PlanFile) -> Self {
        Self {
            id: plan.frontmatter.plan_id.clone(),
            path,
            state: plan.frontmatter.state,
        }
    }

    pub fn is_executing(&self) -> bool {
        matches!(self.state, PlanFileState::Executing)
    }
}
