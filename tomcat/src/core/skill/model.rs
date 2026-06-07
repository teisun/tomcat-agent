use std::collections::BTreeMap;
use std::path::PathBuf;

/// Skill 来源优先级：`Project < Agent < Managed`，first-wins 时高优先级先扫描先入选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillSource {
    Project,
    Agent,
    Managed,
}

impl SkillSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkillSource::Project => "project",
            SkillSource::Agent => "agent",
            SkillSource::Managed => "managed",
        }
    }
}

/// 运行时 Skill 元数据；发现期只缓存目录卡片，不缓存正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub source: SkillSource,
    pub allowed_tools: Option<Vec<String>>,
    pub disable_model_invocation: bool,
}

/// 发现期的坏文件诊断；单文件失败不阻断整批扫描。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiagnostic {
    pub path: PathBuf,
    pub reason: String,
}

/// 进程内 Skill 目录账本：按名字索引元数据，并保留 diagnostics / warnings。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillSet {
    pub by_name: BTreeMap<String, Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
    pub warnings: Vec<String>,
}

impl SkillSet {
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}
