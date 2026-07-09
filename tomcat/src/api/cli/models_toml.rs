//! `tomcat init` 自动生成 / 维护用户级 `~/.tomcat/models.toml`。
//!
//! 当前受管预置条目直接来自内嵌 [`crate::core::llm::catalog::builtin_seed_toml_text`]。
//!
//! 设计要点：
//! - 运行时事实源仍是 [`crate::core::llm::ModelCatalog::load`] 使用的 builtin catalog；本模块只负责
//!   「init 时把同一批预置原样释放到用户文件里」。
//! - **幂等**：文件不存在则创建；已存在则补缺失的受管条目，并为已有受管预置条目补缺失
//!   `model_name`，绝不重写 / 覆盖用户已有条目与注释。
//! - 用户可在 `models.toml` 里直接改这些预置；运行时按「同 id 覆盖 builtin」语义合并。

use std::collections::HashMap;
use std::path::Path;

use crate::core::llm::catalog::{builtin_seed_entries_result, builtin_seed_toml_text};
use crate::core::llm::{ModelCatalog, ModelEntry};
use crate::{AppConfig, AppError};

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeedBlock {
    id: String,
    block: String,
}

/// init 维护 `models.toml` 的结果，用于打印对用户友好的提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelsTomlStatus {
    /// 文件原本不存在，已新建并写入全部受管预置条目。
    Created { added_model_ids: Vec<String> },
    /// 文件已存在，已补缺失条目和/或为现有受管预置条目补缺失的 `model_name`。
    UpdatedExisting {
        added_model_ids: Vec<String>,
        updated_model_name_ids: Vec<String>,
    },
    /// 文件已存在且已含全部受管预置条目，未做任何改动。
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
#   api             走哪条 wire：openai | openai-responses | anthropic-messages
#   provider        逻辑厂商；决定取哪个 <PROVIDER>_API_KEY 环境变量
#   api_key_env     显式凭证变量名；省略时自动推断为 <PROVIDER>_API_KEY
#   base_url        可只填 host，也可带厂商路径；程序会按 api 自动补 leaf
#                   例如 host -> /v1/<leaf>，GLM 这类 /api/paas/v4 路径会被保留
#   thinking_format openai | deepseek | qwen | doubao | anthropic 等
#   capabilities    vision/files/tools/reasoning/web_search 能力位
#   context_window  模型上下文窗口（当前用于列表显示；运行时预算仍读 [context] 全局配置）
#
# API Key 请写入 ~/.tomcat/assets/.env（0600 权限），不要回填到本文件。
# 再加一个模型时，复制下面这段、改 id/provider/base_url 即可。
";

/// 确保 `~/.tomcat/models.toml` 存在且含全部受管预置条目。
pub(crate) fn ensure_default_models_toml(cfg: &AppConfig) -> Result<ModelsTomlStatus, AppError> {
    let path = ModelCatalog::default_user_path(cfg)?;
    let seed_blocks = builtin_seed_blocks()?;
    let seed_entries = seed_entry_map(cfg)?;
    let seed_model_ids = seed_model_ids(&seed_blocks);

    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        let contents = format!("{MODELS_TOML_HEADER}\n{}", builtin_seed_toml_text());
        write_file_atomic(&path, contents.as_bytes())?;
        return Ok(ModelsTomlStatus::Created {
            added_model_ids: seed_model_ids,
        });
    }

    let existing = std::fs::read_to_string(&path).map_err(AppError::Io)?;
    let missing_models = missing_managed_models(&existing, &seed_blocks);
    let (mut updated, updated_model_name_ids) = sync_managed_model_names(&existing, &seed_entries);
    if missing_models.is_empty() && updated_model_name_ids.is_empty() {
        return Ok(ModelsTomlStatus::AlreadyPresent);
    }

    if !missing_models.is_empty() {
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push('\n');
        updated.push_str(&render_entry_blocks(&seed_blocks, &missing_models)?);
    }
    write_file_atomic(&path, updated.as_bytes())?;
    Ok(ModelsTomlStatus::UpdatedExisting {
        added_model_ids: missing_models,
        updated_model_name_ids,
    })
}

fn write_file_atomic(target: &Path, content: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, content).map_err(AppError::Io)?;
    std::fs::rename(&tmp, target).or_else(|_| {
        std::fs::copy(&tmp, target).map_err(AppError::Io)?;
        let _ = std::fs::remove_file(&tmp);
        Ok(())
    })
}

fn render_entry_blocks(
    seed_blocks: &[SeedBlock],
    model_ids: &[String],
) -> Result<String, AppError> {
    model_ids
        .iter()
        .map(|model_id| {
            seed_blocks
                .iter()
                .find(|entry| entry.id == *model_id)
                .ok_or_else(|| {
                    AppError::Config(format!(
                        "内置预置 `{model_id}` 未在 builtin catalog 中定义。"
                    ))
                })
                .map(|entry| entry.block.clone())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|blocks| format!("{}\n", blocks.join("\n\n")))
}

fn missing_managed_models(contents: &str, seed_blocks: &[SeedBlock]) -> Vec<String> {
    seed_blocks
        .iter()
        .map(|entry| entry.id.clone())
        .filter(|entry| !models_file_has_model(contents, entry))
        .collect()
}

fn sync_managed_model_names(
    contents: &str,
    seed_entries: &HashMap<String, ModelEntry>,
) -> (String, Vec<String>) {
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
                if let Some(entry) = seed_entries.get(model_id) {
                    let indent = leading_whitespace(&lines[id_line_index]).to_string();
                    lines.insert(
                        id_line_index + 1,
                        format!("{indent}model_name = \"{}\"", entry.request_model_name()),
                    );
                    if !updated_ids.iter().any(|existing| existing == model_id) {
                        updated_ids.push(model_id.to_string());
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

fn seed_entry_map(cfg: &AppConfig) -> Result<HashMap<String, ModelEntry>, AppError> {
    let builtin_entries = builtin_seed_entries_result(&cfg.context)?
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<HashMap<_, _>>();
    let seed_blocks = builtin_seed_blocks()?;
    for seed_block in &seed_blocks {
        if !builtin_entries.contains_key(&seed_block.id) {
            return Err(AppError::Config(format!(
                "内嵌 builtin_models.toml 包含 `{}`，但 builtin catalog 未定义该模型。",
                seed_block.id
            )));
        }
    }
    if builtin_entries.len() != seed_blocks.len() {
        return Err(AppError::Config(
            "内嵌 builtin_models.toml 与 builtin catalog 的条目数不一致。".to_string(),
        ));
    }
    Ok(builtin_entries)
}

fn builtin_seed_blocks() -> Result<Vec<SeedBlock>, AppError> {
    let text = builtin_seed_toml_text();
    let starts = text
        .match_indices("[[models]]")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return Err(AppError::Config(
            "内嵌 builtin_models.toml 不含任何 [[models]] 块。".to_string(),
        ));
    }

    let mut blocks = Vec::with_capacity(starts.len());
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(text.len());
        let block = text[*start..end].trim().to_string();
        let model_id = block
            .lines()
            .find_map(|line| parse_string_field(line.trim(), "id"))
            .map(str::to_string)
            .ok_or_else(|| {
                AppError::Config(
                    "内嵌 builtin_models.toml 存在缺失 id 的 [[models]] 块。".to_string(),
                )
            })?;
        if blocks
            .iter()
            .any(|existing: &SeedBlock| existing.id == model_id)
        {
            return Err(AppError::Config(format!(
                "内嵌 builtin_models.toml 存在重复模型 id：`{model_id}`。"
            )));
        }
        blocks.push(SeedBlock {
            id: model_id,
            block,
        });
    }
    Ok(blocks)
}

fn seed_model_ids(seed_blocks: &[SeedBlock]) -> Vec<String> {
    seed_blocks.iter().map(|entry| entry.id.clone()).collect()
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
/// 解析失败（用户写坏了文件）时保守返回 `false`，让 init 仅按缺失条目追加受管预置模型，
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
