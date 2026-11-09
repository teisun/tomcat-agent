//! Controller-scoped choices; these are never persisted with a plan or session.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PlanReviewPolicy {
    #[default]
    Ask,
    Auto,
    Skip,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum QuestionPolicy {
    #[default]
    Manual,
    Recommended,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PlanStartPolicy {
    #[default]
    Ask,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionPolicySetting {
    Review(PlanReviewPolicy),
    Questions(QuestionPolicy),
    Start(PlanStartPolicy),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractionPolicy {
    pub review: PlanReviewPolicy,
    pub questions: QuestionPolicy,
    pub start: PlanStartPolicy,
}
