//! `tomcat init` 自动生成 / 维护用户级 `~/.tomcat/models.toml`。
//!
//! 当前受管默认条目：
//! - `mimo-v2.5-pro`
//! - `gpt-5.2`
//! - `deepseek-v4-flash`
//!
//! 设计要点：
//! - 启动加载沿用 [`crate::core::llm::ModelCatalog::load`]，本模块只负责「init 时把文件创建出来」。
//! - **幂等**：文件不存在则创建；已存在则补缺失的受管条目，并为已有受管默认条目补缺失
//!   `model_name`，绝不重写 / 覆盖用户已有条目与注释。
//! - `gpt-5.2` / `deepseek-v4-flash` 的“事实源”现在放在这里生成的 `models.toml`，而非 `builtin_models()`。

use crate::core::llm::ModelCatalog;
use crate::{AppConfig, AppError};

/// init 维护 `models.toml` 的结果，用于打印对用户友好的提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelsTomlStatus {
    /// 文件原本不存在，已新建并写入全部受管默认条目。
    Created { added_model_ids: Vec<&'static str> },
    /// 文件已存在，已补缺失条目和/或为现有受管默认条目补缺失的 `model_name`。
    UpdatedExisting {
        added_model_ids: Vec<&'static str>,
        updated_model_name_ids: Vec<&'static str>,
    },
    /// 文件已存在且已含全部受管默认条目，未做任何改动。
    AlreadyPresent,
}

/// 文件顶部注释：仅在「新建」时写入，避免污染用户已有文件。
const MODELS_TOML_HEADER: &str = "\
# Tomcat 模型清单（models.toml）
#
# 在这里增 / 删 / 改模型，无需改代码、无需重新编译；程序启动会自动把本文件
# 合并进内置模型表（同 id 覆盖内置，新 id 直接新增）。
#
# 字段说明：
#   id              本地模型 id（选择 / 显示用）
#   model_name      上游真实模型名；省略时默认与 id 相同
#   api             走哪条 wire：openai（/v1/chat/completions）| openai-responses（/v1/responses）
#   provider        逻辑厂商；决定取哪个 <PROVIDER>_API_KEY 环境变量
#   api_key_env     显式凭证变量名；省略时自动推断为 <PROVIDER>_API_KEY
#   base_url        只填 host，后缀由 api 决定，不要写成完整 endpoint
#   thinking_format openai | deepseek | qwen | doubao 等
#   capabilities    vision/files/tools/reasoning 能力位
#
# 再加一个模型时，复制下面这段、改 id/provider/base_url 即可。
";

struct ManagedModelTemplate {
    id: &'static str,
    model_name: &'static str,
    entry_block: &'static str,
}

const MANAGED_MODELS: &[ManagedModelTemplate] = &[
    ManagedModelTemplate {
        id: "mimo-v2.5-pro",
        model_name: "mimo-v2.5-pro",
        entry_block: "\
[[models]]
id = \"mimo-v2.5-pro\"
model_name = \"mimo-v2.5-pro\"
api = \"openai\"
provider = \"mimo\"
api_key_env = \"MIMO_API_KEY\"
base_url = \"https://token-plan-cn.xiaomimimo.com\"
thinking_format = \"doubao\"
context_window = 1000000
capabilities = { vision = false, files = false, tools = true, reasoning = true }
",
    },
    ManagedModelTemplate {
        id: "gpt-5.2",
        model_name: "gpt-5.2",
        entry_block: "\
[[models]]
id = \"gpt-5.2\"
model_name = \"gpt-5.2\"
api = \"openai-responses\"
provider = \"openai\"
api_key_env = \"OPENAI_API_KEY\"
base_url = \"https://api.openai.com\"
thinking_format = \"openai\"
capabilities = { vision = true, files = true, tools = true, reasoning = true }
",
    },
    ManagedModelTemplate {
        id: "deepseek-v4-flash",
        model_name: "deepseek-v4-flash",
        entry_block: "\
[[models]]
id = \"deepseek-v4-flash\"
model_name = \"deepseek-v4-flash\"
api = \"openai\"
provider = \"deepseek\"
api_key_env = \"DEEPSEEK_API_KEY\"
base_url = \"https://api.deepseek.com\"
thinking_format = \"deepseek\"
capabilities = { vision = false, files = false, tools = true, reasoning = true }
",
    },
];

/// 确保 `~/.tomcat/models.toml` 存在且含全部受管默认条目。
pub(crate) fn ensure_default_models_toml(cfg: &AppConfig) -> Result<ModelsTomlStatus, AppError> {
    let path = ModelCatalog::default_user_path(cfg)?;

    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        let contents = format!(
            "{MODELS_TOML_HEADER}\n{}",
            render_entry_blocks(MANAGED_MODELS)
        );
        std::fs::write(&path, contents).map_err(AppError::Io)?;
        return Ok(ModelsTomlStatus::Created {
            added_model_ids: managed_model_ids(MANAGED_MODELS),
        });
    }

    let existing = std::fs::read_to_string(&path).map_err(AppError::Io)?;
    let missing_models = missing_managed_models(&existing);
    let (mut updated, updated_model_name_ids) = sync_managed_model_names(&existing);
    if missing_models.is_empty() && updated_model_name_ids.is_empty() {
        return Ok(ModelsTomlStatus::AlreadyPresent);
    }

    if !missing_models.is_empty() {
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push('\n');
        updated.push_str(&render_entry_blocks(&missing_models));
    }
    std::fs::write(&path, updated).map_err(AppError::Io)?;
    Ok(ModelsTomlStatus::UpdatedExisting {
        added_model_ids: managed_model_ids(&missing_models),
        updated_model_name_ids,
    })
}

fn managed_model_ids(models: &[ManagedModelTemplate]) -> Vec<&'static str> {
    models.iter().map(|entry| entry.id).collect()
}

fn render_entry_blocks(models: &[ManagedModelTemplate]) -> String {
    models
        .iter()
        .map(|entry| entry.entry_block)
        .collect::<Vec<_>>()
        .join("\n")
}

fn missing_managed_models(contents: &str) -> Vec<ManagedModelTemplate> {
    MANAGED_MODELS
        .iter()
        .filter(|entry| !models_file_has_model(contents, entry.id))
        .map(|entry| ManagedModelTemplate {
            id: entry.id,
            model_name: entry.model_name,
            entry_block: entry.entry_block,
        })
        .collect()
}

fn sync_managed_model_names(contents: &str) -> (String, Vec<&'static str>) {
    let mut lines = contents
        .split('\n')
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>();
    let mut updated_ids = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        if lines[index].trim() != "[[models]]" {
            index += 1;
            continue;
        }

        let next_block = ((index + 1)..lines.len())
            .find(|candidate| lines[*candidate].trim() == "[[models]]")
            .unwrap_or(lines.len());
        let mut model_id = None;
        let mut id_line_index = None;
        let mut has_model_name = false;

        for (offset, line) in lines[(index + 1)..next_block].iter().enumerate() {
            let line_index = index + 1 + offset;
            let trimmed = line.trim();
            if model_id.is_none() {
                if let Some(value) = parse_string_field(trimmed, "id") {
                    model_id = Some(value.to_string());
                    id_line_index = Some(line_index);
                }
            }
            if parse_string_field(trimmed, "model_name").is_some() {
                has_model_name = true;
            }
        }

        if !has_model_name {
            if let (Some(model_id), Some(id_line_index)) = (model_id.as_deref(), id_line_index) {
                if let Some(template) = managed_model_by_id(model_id) {
                    let indent = leading_whitespace(&lines[id_line_index]).to_string();
                    lines.insert(
                        id_line_index + 1,
                        format!("{indent}model_name = \"{}\"", template.model_name),
                    );
                    if !updated_ids.contains(&template.id) {
                        updated_ids.push(template.id);
                    }
                    index = next_block + 1;
                    continue;
                }
            }
        }

        index = next_block;
    }

    (lines.join("\n"), updated_ids)
}

fn managed_model_by_id(model_id: &str) -> Option<&'static ManagedModelTemplate> {
    MANAGED_MODELS.iter().find(|entry| entry.id == model_id)
}

fn parse_string_field<'a>(line: &'a str, field_name: &str) -> Option<&'a str> {
    let (key, value) = line.split_once('=')?;
    if key.trim() != field_name {
        return None;
    }
    let value = value.trim();
    let start = value.find('"')?;
    let tail = &value[(start + 1)..];
    let end = tail.find('"')?;
    Some(&tail[..end])
}

fn leading_whitespace(line: &str) -> &str {
    let boundary = line
        .find(|c: char| !c.is_whitespace())
        .unwrap_or(line.len());
    &line[..boundary]
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
