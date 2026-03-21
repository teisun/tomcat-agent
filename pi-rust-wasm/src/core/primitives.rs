//! # 4 原语执行引擎 Trait 与类型（与 design CODE_BLOCK_P1_006 一致）

use crate::infra::error::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteFileResult {
    pub path: String,
    pub written: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditFileResult {
    pub path: String,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOperation {
    pub operation_type: EditOperationType,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    pub old_content: Option<String>,
    pub new_content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum EditOperationType {
    Replace,
    Insert,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PrimitiveOperation {
    Read,
    Write,
    Edit,
    Bash,
}

/// 4 原语执行引擎 Trait（与 design CODE_BLOCK_P1_006 一致）。
#[async_trait]
pub trait PrimitiveExecutor: Send + Sync + 'static {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError>;
    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError>;
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        plugin_id: &str,
    ) -> Result<WriteFileResult, AppError>;
    async fn edit_file(
        &self,
        path: &str,
        edits: Vec<EditOperation>,
        plugin_id: &str,
    ) -> Result<EditFileResult, AppError>;
    /// 执行 bash/进程。
    /// - `argv` 为 `None`：`command` 视为完整 shell 命令（经 `sh -c` / `cmd /C`）。
    /// - `argv` 为 `Some`：`command` 为可执行文件名，`argv` 为其参数列表（不经 shell，与 pi-mono `exec(cmd, args)` 对齐）。
    async fn execute_bash(
        &self,
        command: &str,
        cwd: Option<&str>,
        plugin_id: &str,
        argv: Option<&[String]>,
    ) -> Result<BashResult, AppError>;
    async fn require_user_confirmation(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError>;
}
