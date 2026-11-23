//! Role settings are read at dispatch time. Presence of the reviewer table, even
//! when empty, records the transition away from the legacy shared reviewer key.
use super::AppConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Explorer,
    PlanReviewer,
}
impl ModelRole {
    pub fn key(self) -> &'static str {
        match self {
            Self::Explorer => "explorer",
            Self::PlanReviewer => "plan_reviewer",
        }
    }
    pub fn env_override(self) -> Option<String> {
        if self != Self::PlanReviewer || super::saved_configuration_only() {
            return None;
        }
        std::env::var("TOMCAT_REVIEWER_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
    }
    pub fn selected(self, cfg: &AppConfig) -> Option<String> {
        self.env_override()
            .or_else(|| match self {
                Self::Explorer => cfg
                    .subagents
                    .explorer
                    .as_ref()
                    .and_then(|r| r.model.clone()),
                Self::PlanReviewer => match &cfg.subagents.plan_reviewer {
                    Some(r) => r.model.clone(),
                    None => cfg.reviewer.model_override.clone(),
                },
            })
            .filter(|v| !v.trim().is_empty())
    }
}
#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct SubagentConfig {
    pub explorer: Option<RoleConfig>,
    pub plan_reviewer: Option<RoleConfig>,
}
#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct RoleConfig {
    pub model: Option<String>,
}
