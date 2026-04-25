//! System prompt 构建：参考 pi-mono `system-prompt.ts` 模式。
//!
//! prompt 模板写死在代码中，编译后嵌入二进制，不从外部文件读取。
//! 动态部分（当前时间、workspace 路径）在每次调用时填充。
//!
//! ## 模块化
//!
//! `SystemPromptSection` trait 允许注册任意 section，`SystemPromptBuilder`
//! 按 `priority` 升序拼接。`build_system_prompt` 保留为便捷 wrapper。

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
// Convenience wrapper (API-compatible)
// ---------------------------------------------------------------------------

/// 构建发送给 LLM 的 system message 内容。
///
/// 内部使用 `SystemPromptBuilder` 的默认注册（CoreIdentity + ToolInstructions
/// + PagedReading + WorkspaceContext），与旧版输出功能等价。
pub fn build_system_prompt(workspace_dir: &str) -> String {
    SystemPromptBuilder::default().build(workspace_dir)
}
