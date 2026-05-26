//! System prompt 构建：参考 pi-mono `system-prompt.ts` 模式。
//!
//! prompt 模板位于 `core/prompts/templates/`，通过 `include_str!` 编译后嵌入二进制，
//! 不从外部文件读取。
//! 动态部分（当前时间、三类工作目录）在每次调用时填充。
//!
//! ## 模块化
//!
//! `SystemPromptSection` trait 允许注册任意 section，`SystemPromptBuilder`
//! 按 `priority` 升序拼接。`build_system_prompt` 保留为便捷 wrapper。
//!
//! ## `WorkspaceStateSection`（plan §8）
//!
//! `WorkspaceContextSection` 负责解释三类工作目录；`WorkspaceStateSection` 只按权限
//! 分类列出当前可访问目录清单。

// ---------------------------------------------------------------------------
// SystemPromptSection trait + Builder
// ---------------------------------------------------------------------------

use crate::core::prompts::{load as load_prompt, render as render_prompt, PromptKey};

pub trait SystemPromptSection: Send + Sync {
    fn section_name(&self) -> &str;
    fn render(&self, context: &WorkspaceContext) -> String;
    fn priority(&self) -> u32 {
        100
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    pub agent_workspace_dir: String,
    pub agent_definition_dir: String,
    pub agent_plans_dir: String,
    pub agent_trail_dir: String,
}

pub struct SystemPromptBuilder {
    sections: Vec<Box<dyn SystemPromptSection>>,
}

impl SystemPromptBuilder {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    pub fn register(&mut self, section: Box<dyn SystemPromptSection>) {
        self.sections.push(section);
    }

    pub fn build(&self, context: &WorkspaceContext) -> String {
        let mut ordered: Vec<&Box<dyn SystemPromptSection>> = self.sections.iter().collect();
        ordered.sort_by_key(|s| s.priority());
        ordered
            .iter()
            .map(|s| s.render(context))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        let mut builder = Self::new();
        builder.register(Box::new(CoreIdentitySection));
        builder.register(Box::new(ToolInstructionsSection));
        builder.register(Box::new(PagedReadingSection));
        builder.register(Box::new(BackgroundShellMonitorSection));
        builder.register(Box::new(WorkspaceContextSection));
        builder
    }
}

// ---------------------------------------------------------------------------
// Built-in sections
// ---------------------------------------------------------------------------

struct CoreIdentitySection;

impl SystemPromptSection for CoreIdentitySection {
    fn section_name(&self) -> &str {
        "core_identity"
    }
    fn render(&self, _context: &WorkspaceContext) -> String {
        let tool_lines = crate::core::tools::contract::catalog::render_core_identity_tool_lines();
        render_prompt(
            PromptKey::SystemCoreIdentity,
            &[("tool_lines", &tool_lines)],
        )
    }
    fn priority(&self) -> u32 {
        10
    }
}

struct ToolInstructionsSection;

impl SystemPromptSection for ToolInstructionsSection {
    fn section_name(&self) -> &str {
        "tool_instructions"
    }
    fn render(&self, _context: &WorkspaceContext) -> String {
        load_prompt(PromptKey::SystemToolInstructions).to_string()
    }
    fn priority(&self) -> u32 {
        20
    }
}

struct PagedReadingSection;

impl SystemPromptSection for PagedReadingSection {
    fn section_name(&self) -> &str {
        "paged_reading"
    }
    fn render(&self, _context: &WorkspaceContext) -> String {
        load_prompt(PromptKey::SystemPagedReading).to_string()
    }
    fn priority(&self) -> u32 {
        25
    }
}

/// P1（bash background monitor）：教模型如何使用 `bash run_in_background` +
/// `task_output(block=true|false, timeout_ms=...)` 三种模式，以及如何识别
/// `<background-task-finished>` 系统注入的 user message。
struct BackgroundShellMonitorSection;

impl SystemPromptSection for BackgroundShellMonitorSection {
    fn section_name(&self) -> &str {
        "background_shell_monitor"
    }
    fn render(&self, _context: &WorkspaceContext) -> String {
        load_prompt(PromptKey::SystemBackgroundShellMonitor).to_string()
    }
    fn priority(&self) -> u32 {
        30
    }
}

struct WorkspaceContextSection;

impl SystemPromptSection for WorkspaceContextSection {
    fn section_name(&self) -> &str {
        "workspace_context"
    }
    fn render(&self, context: &WorkspaceContext) -> String {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M %Z");
        render_prompt(
            PromptKey::SystemWorkspaceContext,
            &[
                ("now", &now.to_string()),
                ("agent_workspace_dir", &context.agent_workspace_dir),
                ("agent_definition_dir", &context.agent_definition_dir),
                ("agent_plans_dir", &context.agent_plans_dir),
                ("agent_trail_dir", &context.agent_trail_dir),
            ],
        )
    }
    fn priority(&self) -> u32 {
        200
    }
}

// ---------------------------------------------------------------------------
// WorkspaceStateSection（plan §8.1）
// ---------------------------------------------------------------------------

/// 工作区状态快照——`PermissionGate::effective_roots()` 与 `effective_path_rules()`
/// 的精简视图，避免直接耦合 `core::permission::PathBuf` / `PathRule`。
///
/// `read_write` / `read_only` 元素已经过 `expand_tilde` + canonicalize（调用方
/// 负责），用于直接渲染给 LLM。
pub struct WorkspaceState {
    /// 用户可读写的目录列表（含 agent_definition_dir、workspace_roots、session_grants、dragged）。
    pub read_write: Vec<WorkspaceRootDescriptor>,
    /// 仅读目录列表（含 agent_trail_dir 中的 sessions/logs，path_rules readonly 命中等）。
    pub read_only: Vec<WorkspaceRootDescriptor>,
    /// 生效的 path_rules（builtin ∪ user TOML ∪ session 运行时；Deny 全部展示）。
    pub path_rules: Vec<PathRuleSummary>,
}

/// 单条 `read_write` / `read_only` 描述。
pub struct WorkspaceRootDescriptor {
    pub path: String,
    /// 来源标签：`agent_definition_dir` / `agent_workspace_root` /
    /// `session_grant` / `agent_trail_dir` / `path_rule_readonly` 等。
    pub label: String,
    pub alias: Option<String>,
    pub description: Option<String>,
}

/// 单条 path_rule 摘要。
pub struct PathRuleSummary {
    pub path: String,
    /// `"deny"` / `"readonly"`。
    pub mode: String,
    /// `true`：来自 builtin 默认规则；`false`：用户配置或 session 运行时。
    pub builtin: bool,
}

/// `WorkspaceStateSection`：按 plan §8.1 模板渲染。优先级 `150`——
/// `priority` 升序排列，`CoreIdentity(10)` / `ToolInstructions(20)` /
/// `PagedReading(30)` 在前，`WorkspaceContextSection(200)` 在后；
/// `150` 让权限信息在 LLM 看到工具/读取规则之后、当前时间之前出现。
pub struct WorkspaceStateSection {
    state: WorkspaceState,
}

impl WorkspaceStateSection {
    pub fn new(state: WorkspaceState) -> Self {
        Self { state }
    }
}

impl SystemPromptSection for WorkspaceStateSection {
    fn section_name(&self) -> &str {
        "workspace_state"
    }

    fn render(&self, _context: &WorkspaceContext) -> String {
        let read_write_block = if self.state.read_write.is_empty() {
            "You currently have no read/write directories. \
                 Use `config_set(\"workspace.workspace_roots\", \"<abs path>\")` to add one.\n"
                .to_string()
        } else {
            let mut block =
                "You can read/write in these directories (write may require user confirmation):\n"
                    .to_string();
            for (idx, d) in self.state.read_write.iter().enumerate() {
                block.push_str(&format!("  {}. {}", idx + 1, d.path));
                let mut tags: Vec<String> = vec![format!("[{}]", d.label)];
                if let Some(a) = d.alias.as_ref() {
                    tags.push(format!("alias={}", a));
                }
                if let Some(desc) = d.description.as_ref() {
                    tags.push(format!("desc=\"{}\"", desc));
                }
                if !tags.is_empty() {
                    block.push(' ');
                    block.push_str(&tags.join(" "));
                }
                block.push('\n');
            }
            block
        };

        let read_only_block = if self.state.read_only.is_empty() {
            String::new()
        } else {
            let mut block = "\nYou can READ (but NOT write) these directories:\n".to_string();
            for d in &self.state.read_only {
                let suffix = if d.label.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", d.label)
                };
                block.push_str(&format!("  - {}{}\n", d.path, suffix));
            }
            block
        };

        let path_rules_block = if self.state.path_rules.is_empty() {
            String::new()
        } else {
            let mut block = "\nPath rules in effect:\n".to_string();
            // deny 优先列出
            let mut deny: Vec<&PathRuleSummary> = self
                .state
                .path_rules
                .iter()
                .filter(|r| r.mode == "deny")
                .collect();
            let mut readonly: Vec<&PathRuleSummary> = self
                .state
                .path_rules
                .iter()
                .filter(|r| r.mode == "readonly")
                .collect();
            deny.sort_by(|a, b| a.path.cmp(&b.path));
            readonly.sort_by(|a, b| a.path.cmp(&b.path));
            if !deny.is_empty() {
                let lst: Vec<String> = deny
                    .iter()
                    .map(|r| {
                        if r.builtin {
                            format!("{} [builtin]", r.path)
                        } else {
                            r.path.clone()
                        }
                    })
                    .collect();
                block.push_str(&format!("  deny:     {}\n", lst.join(", ")));
            }
            if !readonly.is_empty() {
                let lst: Vec<String> = readonly
                    .iter()
                    .map(|r| {
                        if r.builtin {
                            format!("{} [builtin]", r.path)
                        } else {
                            r.path.clone()
                        }
                    })
                    .collect();
                block.push_str(&format!("  readonly: {}\n", lst.join(", ")));
            }
            block
        };

        render_prompt(
            PromptKey::SystemWorkspaceState,
            &[
                ("read_write_block", &read_write_block),
                ("read_only_block", &read_only_block),
                ("path_rules_block", &path_rules_block),
            ],
        )
    }

    fn priority(&self) -> u32 {
        150
    }
}

// ---------------------------------------------------------------------------
// Convenience wrapper (API-compatible)
// ---------------------------------------------------------------------------

/// 构建发送给 LLM 的 system message 内容。
///
/// 内部使用 `SystemPromptBuilder` 的默认注册（CoreIdentity + ToolInstructions
/// + PagedReading + WorkspaceContext），与旧版输出功能等价。
pub fn build_system_prompt(workspace_dir: &str) -> String {
    let context = WorkspaceContext {
        agent_workspace_dir: workspace_dir.to_string(),
        agent_definition_dir: workspace_dir.to_string(),
        agent_plans_dir: workspace_dir.to_string(),
        agent_trail_dir: workspace_dir.to_string(),
    };
    SystemPromptBuilder::default().build(&context)
}

/// 携带工作区状态的便捷 wrapper（plan §8）：
/// 在默认 section 之上注册 [`WorkspaceStateSection`]，给 Agent 提供权限边界感知。
pub fn build_system_prompt_with_state(context: WorkspaceContext, state: WorkspaceState) -> String {
    let mut builder = SystemPromptBuilder::default();
    builder.register(Box::new(WorkspaceStateSection::new(state)));
    builder.build(&context)
}
