use crate::core::{SessionEntry, TranscriptEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePlan {
    Continue,
}

pub fn compute_resume_plan(_entry: Option<&SessionEntry>, _tail: &[TranscriptEntry]) -> ResumePlan {
    ResumePlan::Continue
}

