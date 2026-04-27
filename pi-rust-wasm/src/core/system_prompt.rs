//! System prompt 构建：参考 pi-mono `system-prompt.ts` 模式。
//!
//! prompt 模板写死在代码中，编译后嵌入二进制，不从外部文件读取。
//! 动态部分（当前时间、workspace 路径）在每次调用时填充。
//!
//! ## 模块化
//!
//! `SystemPromptSection` trait 允许注册任意 section，`SystemPromptBuilder`
//! 按 `priority` 升序拼接。`build_system_prompt` 保留为便捷 wrapper。
//!
//! ## `WorkspaceStateSection`（plan §8）
//!
//! 工作区权限分级落地后，新增 `WorkspaceStateSection`：把
//! `effective_roots`（read_write + read_only）、生效 path_rules、
//! agent_data_dir、config 工具引导一次性渲染进 system prompt，
//! 让 Agent 第 0 轮就能正确回答"我可以读哪些目录？"。

// ---------------------------------------------------------------------------
// SystemPromptSection trait + Builder
// ---------------------------------------------------------------------------

pub trait SystemPromptSection: Send + Sync {
    fn section_name(&self) -> &str;
    fn render(&self, workspace_dir: &str) -> String;
    fn priority(&self) -> u32 {
        100
    }
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

    pub fn build(&self, workspace_dir: &str) -> String {
        let mut ordered: Vec<&Box<dyn SystemPromptSection>> = self.sections.iter().collect();
        ordered.sort_by_key(|s| s.priority());
        ordered
            .iter()
            .map(|s| s.render(workspace_dir))
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
    fn render(&self, _workspace_dir: &str) -> String {
        r#"You are an expert coding assistant operating inside pi-wasm, a coding agent runtime.
You help users by reading files, executing commands, editing code, and writing new files.

Available tools:
- read_file: Read file contents
- write_file: Create or overwrite files
- edit_file: Make surgical edits to files (find exact text and replace with new text)
- execute_bash: Execute bash commands
- list_dir: List directory contents"#
            .to_string()
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
    fn render(&self, _workspace_dir: &str) -> String {
        r#"Guidelines:
- When users ask you to write, edit, or create files, proactively use the tools above to do it directly — do not just explain how
- Use read_file to examine files before editing
- Use edit_file for precise changes (old_content must match the file exactly, including whitespace)
- Use write_file only for new files or complete rewrites
- Be concise in your responses
- Show file paths clearly when working with files
- IMPORTANT: Only claim you can access directories that you have successfully listed or read from using tools. Do not guess or fabricate which directories are accessible. If unsure, use list_dir to verify first."#.to_string()
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
    fn render(&self, _workspace_dir: &str) -> String {
        r#"- When you see "[Tool result persisted: <path>]", the original content has been saved to disk.
  You can read specific portions using read_file with offset and limit parameters.
  Do NOT re-read the entire file; read only the relevant sections you need."#
            .to_string()
    }
    fn priority(&self) -> u32 {
        25
    }
}

struct WorkspaceContextSection;

impl SystemPromptSection for WorkspaceContextSection {
    fn section_name(&self) -> &str {
        "workspace_context"
    }
    fn render(&self, workspace_dir: &str) -> String {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M %Z");
        format!("Current date and time: {now}\nCurrent working directory: {workspace_dir}")
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
    /// `pi chat` 启动时 [`std::env::current_dir`] 的快照，绝对路径字符串。
    /// 即使 cwd 没有列入 read_write，也会出现在 system prompt 的「## Current
    /// Working Directory」段，让 LLM 优先在 cwd 下查找/操作；
    /// 是否被授权由 `read_write` / `read_only` 决定，二者解耦。
    /// 旧调用方（仅传入 `read_write` / `read_only` / `path_rules` / `agent_data_dir`
    /// 字段）对本字段不可见，赋默认空串即可，渲染会跳过该段。
    pub cwd: String,
    /// 用户可读写的目录列表（含 workspace_dir、extra_roots、session_grants、dragged）。
    pub read_write: Vec<WorkspaceRootDescriptor>,
    /// 仅读目录列表（含 agent_data_dir 中的 sessions/logs，path_rules readonly 命中等）。
    pub read_only: Vec<WorkspaceRootDescriptor>,
    /// 生效的 path_rules（builtin ∪ user TOML ∪ session 运行时；Deny 全部展示）。
    pub path_rules: Vec<PathRuleSummary>,
    /// agent 凭据/历史目录（plan §9.x）；展示为只读。`None` 时不渲染对应行。
    pub agent_data_dir: Option<String>,
}

/// 单条 `read_write` / `read_only` 描述。
pub struct WorkspaceRootDescriptor {
    pub path: String,
    /// 来源标签：`agent_workspace` / `extra_root` / `session_grant` / `dragged_path` /
    /// `agent_data_dir` / `path_rule_readonly` 等。
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

    fn render(&self, _workspace_dir: &str) -> String {
        let mut out = String::new();

        // ── ## Current Working Directory ──
        // 让 LLM 在每次推理首屏看到「用户启动 pi chat 时所在的目录」。即使 cwd
        // 当前不在 read_write / read_only 里（首次访问会触发 lazy 授权弹窗），
        // 也要让 LLM 知道这是用户的语境根，优先在此处查找/操作；
        // 同时把「访问需要授权」的事实显式说明，避免 LLM 在拒绝时混乱。
        if !self.state.cwd.is_empty() {
            let in_rw = self
                .state
                .read_write
                .iter()
                .any(|d| d.path == self.state.cwd);
            let in_ro = self
                .state
                .read_only
                .iter()
                .any(|d| d.path == self.state.cwd);
            out.push_str("## Current Working Directory\n\n");
            out.push_str(&format!("`{}`\n\n", self.state.cwd));
            if in_rw {
                out.push_str(
                    "This directory is currently writable for you (see Workspace State below).\n",
                );
            } else if in_ro {
                out.push_str(
                    "This directory is currently read-only for you (see Workspace State below).\n",
                );
            } else {
                out.push_str(
                    "This directory is NOT yet authorized. \
                     The user is interacting from here, so prefer interpreting relative paths and \
                     ambiguous references against this directory. \
                     The first time you call a tool that touches a path inside this directory, \
                     the runtime will ask the user how to authorize it (one-time / persist-extra-root / deny).\n",
                );
            }
            out.push('\n');
        }

        out.push_str("## Workspace State\n\n");

        if self.state.read_write.is_empty() {
            out.push_str(
                "You currently have no read/write directories. \
                 Use `config_set(\"workspace.extra_roots\", \"<abs path>\")` to add one.\n",
            );
        } else {
            out.push_str(
                "You can read/write in these directories (write may require user confirmation):\n",
            );
            for (idx, d) in self.state.read_write.iter().enumerate() {
                out.push_str(&format!("  {}. {}", idx + 1, d.path));
                let mut tags: Vec<String> = vec![format!("[{}]", d.label)];
                if let Some(a) = d.alias.as_ref() {
                    tags.push(format!("alias={}", a));
                }
                if let Some(desc) = d.description.as_ref() {
                    tags.push(format!("desc=\"{}\"", desc));
                }
                if !tags.is_empty() {
                    out.push(' ');
                    out.push_str(&tags.join(" "));
                }
                out.push('\n');
            }
        }

        if !self.state.read_only.is_empty() {
            out.push_str("\nYou can READ (but NOT write) these directories:\n");
            for d in &self.state.read_only {
                let suffix = if d.label.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", d.label)
                };
                out.push_str(&format!("  - {}{}\n", d.path, suffix));
            }
        }

        if let Some(ref ad) = self.state.agent_data_dir {
            // 当 read_only 已经包含 agent_data_dir 时不重复渲染。
            if !self.state.read_only.iter().any(|d| d.path == *ad) {
                out.push_str(&format!(
                    "\nAgent data dir (read-only, history/logs/audit/profile): {}\n",
                    ad
                ));
            }
        }

        if !self.state.path_rules.is_empty() {
            out.push_str("\nPath rules in effect:\n");
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
                out.push_str(&format!("  deny:     {}\n", lst.join(", ")));
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
                out.push_str(&format!("  readonly: {}\n", lst.join(", ")));
            }
        }

        out.push_str(
            "\nConfiguration management:\n  - To inspect or modify workspace/permissions, use the `config_get` and `config_set` tools.\n  - These tools enforce a key allowlist (sensitive keys like API keys are blocked).\n  - Array configs (extra_roots, path_rules, bash_*) are append-only via tools.\n  - DO NOT write to ~/.pi_/pi.config.toml directly with write_file/edit_file (will be denied).\n",
        );

        out
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
    SystemPromptBuilder::default().build(workspace_dir)
}

/// 携带工作区状态的便捷 wrapper（plan §8）：
/// 在默认 section 之上注册 [`WorkspaceStateSection`]，给 Agent 提供权限边界感知。
pub fn build_system_prompt_with_state(workspace_dir: &str, state: WorkspaceState) -> String {
    let mut builder = SystemPromptBuilder::default();
    builder.register(Box::new(WorkspaceStateSection::new(state)));
    builder.build(workspace_dir)
}
