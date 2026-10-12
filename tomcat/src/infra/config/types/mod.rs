//! 配置类型目录模块：拆分原 `types.rs`，保持对外导出面不变。

mod connector;
mod context;
mod core;
mod llm;
mod primitive;
mod runtime;
mod skills;
mod tools;
mod ui;

pub use connector::*;
pub use context::*;
pub use core::*;
pub use llm::*;
pub use primitive::*;
pub use runtime::*;
pub use skills::*;
pub use tools::*;
pub use ui::*;

use serde::{Deserialize, Serialize};

/// 应用顶层配置，聚合 log / llm / storage / agent / plugin / security / primitive 等子配置。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub preflight: PreflightConfig,
    #[serde(default)]
    pub checkpoint: CheckpointConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub primitive: PrimitiveConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    /// Optional integrations that expose external capabilities as Agent tools.
    #[serde(default)]
    pub connector: ConnectorConfig,
    /// PLAN 模式运行时全局参数（T2-P1-002 PR-PLA/PLB）。
    #[serde(default)]
    pub plan: PlanConfig,
    /// reviewer 内联子 Agent 派发参数（T2-P1-004 RV-A/B/E）。
    #[serde(default)]
    pub reviewer: ReviewerConfig,
    /// session-local todos 工具参数（GAP-N12 / G3）。
    #[serde(default)]
    pub todos: TodosConfig,
    /// `tomcat chat` 启动像素风吉祥物 Splash 配置。
    #[serde(default)]
    pub splash: SplashConfig,
    /// `tomcat serve --stdio` 子进程网关配置。
    #[serde(default)]
    pub serve: ServeConfig,
}
