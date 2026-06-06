use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const ALLOWED_SKILL_FRONTMATTER: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
    "disable-model-invocation",
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SkillParseError {
    #[error("skill 文件缺少 frontmatter 分隔符 ---")]
    FrontmatterDelimMissing,
    #[error("skill frontmatter YAML 解析失败: {0}")]
    YamlParse(String),
    #[error("skill frontmatter 缺少必填字段: {field}")]
    MissingField { field: &'static str },
    #[error("skill.name 非法: {reason}")]
    InvalidName { reason: String },
    #[error("skill.description 非法: {reason}")]
    InvalidDescription { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_yaml::Value>,
    #[serde(
        rename = "allowed-tools",
        default,
        deserialize_with = "deserialize_allowed_tools"
    )]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawSkillFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, serde_yaml::Value>,
    #[serde(
        rename = "allowed-tools",
        default,
        deserialize_with = "deserialize_allowed_tools"
    )]
    allowed_tools: Option<Vec<String>>,
    #[serde(rename = "disable-model-invocation", default)]
    disable_model_invocation: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
enum AllowedToolsField {
    One(String),
    Many(Vec<String>),
}

fn deserialize_allowed_tools<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<AllowedToolsField>::deserialize(deserializer)?;
    Ok(raw.and_then(|value| match value {
        AllowedToolsField::One(tool) => {
            let trimmed = tool.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(vec![trimmed.to_string()])
            }
        }
        AllowedToolsField::Many(tools) => {
            let normalized = tools
                .into_iter()
                .map(|tool| tool.trim().to_string())
                .filter(|tool| !tool.is_empty())
                .collect::<Vec<_>>();
            if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            }
        }
    }))
}

pub fn parse(text: &str) -> Result<SkillFrontmatter, SkillParseError> {
    let (yaml, _) = split_frontmatter(text)?;
    let raw: RawSkillFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| SkillParseError::YamlParse(e.to_string()))?;
    let name = raw
        .name
        .ok_or(SkillParseError::MissingField { field: "name" })?;
    let description = raw.description.ok_or(SkillParseError::MissingField {
        field: "description",
    })?;
    validate_name(&name)?;
    validate_description(&description)?;
    Ok(SkillFrontmatter {
        name,
        description,
        license: raw.license,
        compatibility: raw.compatibility,
        metadata: raw.metadata,
        allowed_tools: raw.allowed_tools,
        disable_model_invocation: raw.disable_model_invocation,
    })
}

pub fn strip_frontmatter(text: &str) -> Result<&str, SkillParseError> {
    split_frontmatter(text).map(|(_, body)| body)
}

pub fn split_frontmatter(text: &str) -> Result<(&str, &str), SkillParseError> {
    let (mut next, first_line) =
        next_line(text, 0).ok_or(SkillParseError::FrontmatterDelimMissing)?;
    if first_line != "---" {
        return Err(SkillParseError::FrontmatterDelimMissing);
    }
    let yaml_start = next;
    loop {
        let line_start = next;
        let (after_line, line) =
            next_line(text, next).ok_or(SkillParseError::FrontmatterDelimMissing)?;
        if line == "---" {
            return Ok((&text[yaml_start..line_start], &text[after_line..]));
        }
        next = after_line;
    }
}

fn next_line(text: &str, start: usize) -> Option<(usize, &str)> {
    if start >= text.len() {
        return None;
    }
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    let line_end = if end > start && bytes[end.saturating_sub(1)] == b'\r' {
        end - 1
    } else {
        end
    };
    let next = if end < bytes.len() { end + 1 } else { end };
    Some((next, &text[start..line_end]))
}

fn validate_name(name: &str) -> Result<(), SkillParseError> {
    if name.is_empty() {
        return Err(SkillParseError::MissingField { field: "name" });
    }
    if name.len() > 64 {
        return Err(SkillParseError::InvalidName {
            reason: "长度必须 <= 64".to_string(),
        });
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(SkillParseError::InvalidName {
            reason: "仅允许 [a-z0-9-]".to_string(),
        });
    }
    Ok(())
}

fn validate_description(description: &str) -> Result<(), SkillParseError> {
    if description.trim().is_empty() {
        return Err(SkillParseError::MissingField {
            field: "description",
        });
    }
    if description.len() > 1024 {
        return Err(SkillParseError::InvalidDescription {
            reason: "长度必须 <= 1024".to_string(),
        });
    }
    Ok(())
}
