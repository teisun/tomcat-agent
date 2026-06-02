//! `tomcat init` 自动生成 / 维护用户级 `~/.tomcat/models.toml`。
//!
//! 设计要点（见接入计划 §5.5）：
//! - 启动加载沿用 [`crate::core::llm::ModelCatalog::load`]，本模块只负责「init 时把文件创建出来」。
//! - **幂等**：文件不存在则创建；已存在则仅在缺 `mimo-v2.5-pro` 时**追加**，
//!   绝不重写 / 覆盖用户已有条目与注释。
//! - MiMo 的「事实源」放在这里生成的 `models.toml`，而非 `builtin_models()`，
//!   因此它同时是「零代码加模型」的活样板。

use crate::core::llm::ModelCatalog;
use crate::{AppConfig, AppError};

/// init 维护 `models.toml` 的结果，用于打印对用户友好的提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelsTomlStatus {
    /// 文件原本不存在，已新建并写入 MiMo 样板。
    Created,
    /// 文件已存在但缺 MiMo，已仅追加 MiMo 条目（保留其余内容）。
    AppendedMimo,
    /// 文件已存在且已含 MiMo，未做任何改动。
    AlreadyPresent,
}

const MIMO_MODEL_ID: &str = "mimo-v2.5-pro";

/// 文件顶部注释：仅在「新建」时写入，避免污染用户已有文件。
const MODELS_TOML_HEADER: &str = "\
# Tomcat 模型清单（models.toml）
#
# 在这里增 / 删 / 改模型，无需改代码、无需重新编译；程序启动会自动把本文件
# 合并进内置模型表（同 id 覆盖内置，新 id 直接新增）。
#
# 字段说明：
#   id              模型 id（请求里用的名字）
#   api             走哪条 wire：openai（/v1/chat/completions）| openai-responses（/v1/responses）
#   provider        逻辑厂商；决定取哪个 <PROVIDER>_API_KEY 环境变量
#   base_url        只填 host，后缀由 api 决定，不要写成完整 endpoint
#   thinking_format openai | deepseek | qwen | doubao 等
#   capabilities    vision/files/tools/reasoning 能力位
#
# 再加一个模型时，复制下面这段、改 id/provider/base_url 即可。
";

/// MiMo 条目文本块；新建与追加共用同一份，保证幂等一致。
const MIMO_ENTRY_BLOCK: &str = "\
[[models]]
id = \"mimo-v2.5-pro\"
api = \"openai\"
provider = \"mimo\"
base_url = \"https://token-plan-cn.xiaomimimo.com\"
thinking_format = \"doubao\"
context_window = 1000000
capabilities = { vision = false, files = false, tools = true, reasoning = true }
";

/// 确保 `~/.tomcat/models.toml` 存在且含可用的 `mimo-v2.5-pro` 条目。
///
/// 幂等：
/// - 不存在 → 写 header + MiMo 条目，返回 [`ModelsTomlStatus::Created`]；
/// - 存在但缺 MiMo → 仅在文件尾部追加 MiMo 条目，返回 [`ModelsTomlStatus::AppendedMimo`]；
/// - 存在且已含 MiMo → 不改动，返回 [`ModelsTomlStatus::AlreadyPresent`]。
pub(crate) fn ensure_mimo_models_toml(cfg: &AppConfig) -> Result<ModelsTomlStatus, AppError> {
    let path = ModelCatalog::default_user_path(cfg)?;

    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        let contents = format!("{MODELS_TOML_HEADER}\n{MIMO_ENTRY_BLOCK}");
        std::fs::write(&path, contents).map_err(AppError::Io)?;
        return Ok(ModelsTomlStatus::Created);
    }

    let existing = std::fs::read_to_string(&path).map_err(AppError::Io)?;
    if models_file_has_model(&existing, MIMO_MODEL_ID) {
        return Ok(ModelsTomlStatus::AlreadyPresent);
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push('\n');
    updated.push_str(MIMO_ENTRY_BLOCK);
    std::fs::write(&path, updated).map_err(AppError::Io)?;
    Ok(ModelsTomlStatus::AppendedMimo)
}

/// 解析现有 `models.toml`，判断是否已含某个 model id。
///
/// 解析失败（用户写坏了文件）时保守返回 `false`，让 init 追加一条可用 MiMo，
/// 同时绝不触碰用户原有文本。
fn models_file_has_model(contents: &str, model_id: &str) -> bool {
    let Ok(value) = toml::from_str::<toml::Value>(contents) else {
        return false;
    };
    value
        .get("models")
        .and_then(toml::Value::as_array)
        .map(|models| {
            models.iter().any(|entry| {
                entry
                    .get("id")
                    .and_then(toml::Value::as_str)
                    .map(|id| id.trim() == model_id)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "tests/models_toml_test.rs"]
mod tests;
