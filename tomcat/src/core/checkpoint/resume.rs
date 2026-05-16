use crate::core::{SessionEntry, TranscriptEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePlan {
    Continue,
}

pub fn compute_resume_plan(_entry: Option<&SessionEntry>, _tail: &[TranscriptEntry]) -> ResumePlan {
    ResumePlan::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_plan_always_continue() {
        assert!(matches!(
            compute_resume_plan(None, &[]),
            ResumePlan::Continue
        ));
    }
}
