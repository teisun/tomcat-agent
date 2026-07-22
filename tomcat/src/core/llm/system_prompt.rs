//! System prompt 构建：参考 pi-mono `system-prompt.ts` 模式。
//!
//! prompt 模板位于 `core/prompts/templates/`，通过 `include_str!` 编译后嵌入二进制，
//! 不从外部文件读取。
//! 动态工作目录在每次调用时填充。
//!
//! ## 模块化
//!
//! `SystemPromptSection` trait 允许注册任意 section，`SystemPromptBuilder`
//! 按 `priority` 升序拼接。`build_system_prompt` 保留为便捷 wrapper。
//!
//! 默认 section 链（priority）：`CoreIdentity(10)` → `ToolInstructions(20)` →
//! `OutputConventions(21)` → `ParallelTools(22)` → `PagedReading(25)` → `BackgroundShellMonitor(30)` →
//! `Verification(50)` →（可选 `AvailableSkills(35)`）→ `WorkspaceState(150)` →
//! `WorkspaceContext(200)`。
//!
//! ## 跨工具规则注入
//!
//! `ToolInstructionsSection` 渲染时把 `catalog::render_tool_guidelines_with_policy`
//! 聚合去重后的跨工具规则注入 `tool_instructions.txt` 的 `{tool_guidelines}` 占位，
//! 使每条规则只说一遍（read-before-edit / 别粘显示前缀 / 优先 search_files /
//! `path:line` 引用 / 别用 codeblock 假编辑 / UI 以 UX 为先 / 防幻觉）。
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
    pub tool_lines: Option<String>,
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
        builder.register(Box::new(OutputConventionsSection));
        builder.register(Box::new(ParallelToolsSection));
        builder.register(Box::new(PagedReadingSection));
        builder.register(Box::new(BackgroundShellMonitorSection));
        builder.register(Box::new(VerificationSection));
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
        let tool_lines = _context.tool_lines.clone().unwrap_or_else(|| {
            crate::core::tools::contract::catalog::render_core_identity_tool_lines_with_policy(
                false,
            )
        });
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
        // 跨工具规则从 catalog 的 prompt_guidelines 聚合去重后注入 {tool_guidelines}，
        // 只说一遍，避免在每条 description 里逐工具重复（见 catalog.rs 成功率红线注释）。
        let guidelines =
            crate::core::tools::contract::catalog::render_tool_guidelines_with_policy(true);
        render_prompt(
            PromptKey::SystemToolInstructions,
            &[("tool_guidelines", &guidelines)],
        )
    }
    fn priority(&self) -> u32 {
        20
    }
}

struct OutputConventionsSection;

impl SystemPromptSection for OutputConventionsSection {
    fn section_name(&self) -> &str {
        "output_conventions"
    }
    fn render(&self, _context: &WorkspaceContext) -> String {
        load_prompt(PromptKey::SystemOutputConventions).to_string()
    }
    fn priority(&self) -> u32 {
        21
    }
}

/// 并行工具调用引导（priority 22）：无依赖的调用同轮批量发出，省 LLM 往返 + 上下文重发。
struct ParallelToolsSection;

impl SystemPromptSection for ParallelToolsSection {
    fn section_name(&self) -> &str {
        "parallel_tools"
    }
    fn render(&self, _context: &WorkspaceContext) -> String {
        load_prompt(PromptKey::SystemParallelTools).to_string()
    }
    fn priority(&self) -> u32 {
        22
    }
}

/// 收尾 + 验证引导（priority 50）：finish-the-job + 反捏造 + 引用 EXEC Mini 验证。
struct VerificationSection;

impl SystemPromptSection for VerificationSection {
    fn section_name(&self) -> &str {
        "verification"
    }
    fn render(&self, _context: &WorkspaceContext) -> String {
        load_prompt(PromptKey::SystemVerification).to_string()
    }
    fn priority(&self) -> u32 {
        50
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

pub struct AvailableSkillsSection {
    rendered: String,
}

pub fn render_available_skills_prompt(
    skill_set: &crate::core::skill::SkillSet,
    context_budget_chars: usize,
    cfg: &crate::infra::config::SkillsConfig,
) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let total_budget =
        crate::core::skill::compute_skill_prompt_budget_chars(context_budget_chars, cfg);
    let overhead = render_prompt(PromptKey::SystemAvailableSkills, &[("skills_block", "")]).len();
    let block_budget = total_budget.saturating_sub(overhead);
    let rendered = crate::core::skill::render_available_skills_block(
        skill_set,
        block_budget,
        cfg.max_description_chars,
    );
    if rendered.block.trim().is_empty() {
        None
    } else {
        Some(render_prompt(
            PromptKey::SystemAvailableSkills,
            &[("skills_block", &rendered.block)],
        ))
    }
}

impl AvailableSkillsSection {
    pub fn from_skill_set(
        skill_set: &crate::core::skill::SkillSet,
        context_budget_chars: usize,
        cfg: &crate::infra::config::SkillsConfig,
    ) -> Option<Self> {
        render_available_skills_prompt(skill_set, context_budget_chars, cfg)
            .map(|rendered| Self { rendered })
    }
}

impl SystemPromptSection for AvailableSkillsSection {
    fn section_name(&self) -> &str {
        "available_skills"
    }

    fn render(&self, _context: &WorkspaceContext) -> String {
        self.rendered.clone()
    }

    fn priority(&self) -> u32 {
        35
    }
}

struct WorkspaceContextSection;

impl SystemPromptSection for WorkspaceContextSection {
    fn section_name(&self) -> &str {
        "workspace_context"
    }
    fn render(&self, context: &WorkspaceContext) -> String {
        render_prompt(
            PromptKey::SystemWorkspaceContext,
            &[
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
/// `150` 让权限信息在 LLM 看到工具/读取规则之后、工作目录上下文之前出现。
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
/// + OutputConventions + ParallelTools + PagedReading + BackgroundShellMonitor
/// + Verification + WorkspaceContext）。
pub fn build_system_prompt(workspace_dir: &str) -> String {
    let context = WorkspaceContext {
        agent_workspace_dir: workspace_dir.to_string(),
        agent_definition_dir: workspace_dir.to_string(),
        agent_plans_dir: workspace_dir.to_string(),
        agent_trail_dir: workspace_dir.to_string(),
        tool_lines: None,
    };
    SystemPromptBuilder::default().build(&context)
}

/// 携带工作区状态的便捷 wrapper（plan §8）：
/// 在默认 section 之上注册 [`WorkspaceStateSection`]，给 Agent 提供权限边界感知。
pub fn build_system_prompt_with_state(context: WorkspaceContext, state: WorkspaceState) -> String {
    build_system_prompt_with_state_and_skills(context, state, None, None, 0)
}

pub fn build_system_prompt_with_state_and_skills(
    context: WorkspaceContext,
    state: WorkspaceState,
    skill_set: Option<&crate::core::skill::SkillSet>,
    skill_cfg: Option<&crate::infra::config::SkillsConfig>,
    context_budget_chars: usize,
) -> String {
    let mut builder = SystemPromptBuilder::default();
    builder.register(Box::new(WorkspaceStateSection::new(state)));
    if let (Some(skill_set), Some(skill_cfg)) = (skill_set, skill_cfg) {
        if let Some(section) =
            AvailableSkillsSection::from_skill_set(skill_set, context_budget_chars, skill_cfg)
        {
            builder.register(Box::new(section));
        }
    }
    builder.build(&context)
}
