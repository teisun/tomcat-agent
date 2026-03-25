//! System prompt 构建：参考 pi-mono `system-prompt.ts` 模式。
//!
//! prompt 模板写死在代码中，编译后嵌入二进制，不从外部文件读取。
//! 动态部分（当前时间、workspace 路径）在每次调用时填充。

/// 构建发送给 LLM 的 system message 内容。
///
/// # 参数
/// - `workspace_dir`：Agent 当前工作目录，告知 LLM 可操作的文件路径范围。
///
/// # 模板说明
/// 参考 pi-mono 的 `buildSystemPrompt()` 模式：
/// - 角色定义：告知 LLM 它是编程 Agent
/// - 可用工具列表及一句话描述
/// - 使用准则（Guidelines）
/// - 动态填充：当前时间 + 工作目录
pub fn build_system_prompt(workspace_dir: &str) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M %Z");
    format!(
        r#"You are an expert coding assistant operating inside pi-wasm, a coding agent runtime.
You help users by reading files, executing commands, editing code, and writing new files.

Available tools:
- read_file: Read file contents
- write_file: Create or overwrite files
- edit_file: Make surgical edits to files (find exact text and replace with new text)
- execute_bash: Execute bash commands
- list_dir: List directory contents

Guidelines:
- When users ask you to write, edit, or create files, proactively use the tools above to do it directly — do not just explain how
- Use read_file to examine files before editing
- Use edit_file for precise changes (old_content must match the file exactly, including whitespace)
- Use write_file only for new files or complete rewrites
- Be concise in your responses
- Show file paths clearly when working with files
- IMPORTANT: Only claim you can access directories that you have successfully listed or read from using tools. Do not guess or fabricate which directories are accessible. If unsure, use list_dir to verify first.

Current date and time: {now}
Current working directory: {workspace_dir}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_system_prompt_contains_tools_and_workspace() {
        let prompt = build_system_prompt("/home/user/workspace");
        assert!(prompt.contains("read_file"));
        assert!(prompt.contains("write_file"));
        assert!(prompt.contains("edit_file"));
        assert!(prompt.contains("execute_bash"));
        assert!(prompt.contains("list_dir"));
        assert!(prompt.contains("/home/user/workspace"));
        assert!(prompt.contains("coding assistant"));
    }

    #[test]
    fn build_system_prompt_contains_current_time() {
        let prompt = build_system_prompt("/tmp");
        assert!(prompt.contains("Current date and time:"));
        assert!(prompt.contains("Current working directory:"));
    }

    #[test]
    fn build_system_prompt_contains_anti_hallucination_constraint() {
        let prompt = build_system_prompt("/tmp");
        assert!(
            prompt.contains("Only claim you can access"),
            "system prompt 应包含防幻觉约束"
        );
    }
}
