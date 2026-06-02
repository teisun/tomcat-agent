//! CLI 子命令：init、doctor、config、session、plugin、audit；无参默认 chat。

mod audit_cmd;
mod chat_cmd;
mod config_cmd;
mod init;
mod init_model_wizard;
mod models_toml;
mod pathrules_cmd;
mod plugin_cmd;
mod session_cmd;
mod workspace_cmd;

#[cfg(test)]
mod tests;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, init_logging, load_config,
    normalize_path, resolve_log_dir, validate_config, AppError, CLI_NAME, DEFAULT_CONFIG_PATH,
};

pub(crate) use audit_cmd::run_audit;
pub(crate) use config_cmd::{config_file_path, run_config};
pub(crate) use init::{run_doctor, run_init};
pub(crate) use pathrules_cmd::run_pathrules;
pub(crate) use plugin_cmd::run_plugin;
pub(crate) use session_cmd::run_session;
pub(crate) use workspace_cmd::run_workspace;

use chat_cmd::run_chat;

#[cfg(test)]
use crate::AppConfig;
#[cfg(test)]
pub(crate) use audit_cmd::{parse_audit_line, read_audit_entries};
#[cfg(test)]
pub(crate) use config_cmd::{resolve_toml_key, set_toml_key};
#[cfg(test)]
pub(crate) use init_model_wizard::{apply_model_choice, write_env_entries};
#[cfg(test)]
pub(crate) use plugin_cmd::{
    load_plugin_registry, save_plugin_registry, PluginRegistryEntry, PluginRegistryFile,
};

/// tomcat CLI：AI Agent 运行时，支持插件管理、会话、配置、审计与对话模式
#[derive(Parser, Debug)]
#[command(
    name = CLI_NAME,
    about = "Tomcat Agent CLI — 插件化 AI Agent 运行时",
    long_about = "tomcat 是基于 WasmEdge + QuickJS 的插件化 AI Agent 运行时。\n支持 init/doctor/config/session/plugin/audit 子命令，无参数时进入对话模式。",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 初始化配置，引导 LLM 与安全策略，生成配置文件
    Init,
    /// 检测运行环境、WasmEdge/QuickJS、配置合法性，输出修复建议
    Doctor,
    /// 配置管理：get/set/edit
    Config {
        #[command(subcommand)]
        sub: ConfigSub,
    },
    /// 会话管理：list/new/switch/delete/archive/search
    Session {
        #[command(subcommand)]
        sub: SessionSub,
    },
    /// 插件管理：list/load/unload/enable/disable/info
    Plugin {
        #[command(subcommand)]
        sub: PluginSub,
    },
    /// 审计日志：list/show/export
    Audit {
        #[command(subcommand)]
        sub: AuditSub,
    },
    /// 工作区管理：add/list/remove
    Workspace {
        #[command(subcommand)]
        sub: WorkspaceSub,
    },
    /// 路径规则管理：add/list（plan §9 / PR-10）
    Pathrules {
        #[command(subcommand)]
        sub: PathRulesSub,
    },
    /// 进入交互式对话模式
    Chat {
        /// 恢复上次会话（默认行为，显式语义）
        #[arg(long, default_value_t = false)]
        resume: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigSub {
    /// 获取配置项（无 key 时输出完整配置）
    Get {
        /// 配置项路径，如 log.level、llm.default_model
        key: Option<String>,
    },
    /// 设置配置项（自动校验合法性）
    Set {
        /// 配置项路径，如 log.level、security.audit_log_retention_days
        key: String,
        /// 新值（自动推断类型：整数/布尔/字符串）
        value: String,
    },
    /// 用编辑器打开配置文件（读取 $EDITOR，默认 vi/notepad）
    Edit,
}

#[derive(Subcommand, Debug)]
pub enum SessionSub {
    /// 列出所有会话
    List,
    /// 创建新会话
    New,
    /// 切换到指定 session_id
    Switch { session_id: String },
    /// 删除会话
    Delete { key: String },
    /// 归档会话
    Archive { key: String },
    /// 搜索会话（MVP 占位：仅列出）
    Search { query: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceSub {
    /// 添加工作区目录
    Add {
        /// 要添加的目录路径（与 --cwd 二选一）
        path: Option<String>,
        /// 将当前工作目录加入授权列表
        #[arg(long)]
        cwd: bool,
    },
    /// 列出已授权的工作区
    List,
    /// 移除工作区目录
    Remove {
        /// 要移除的目录路径
        path: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PathRulesSub {
    /// 追加一条 path_rule（与 `tomcat config set primitive.path_rules <json>` 等价）
    Add {
        /// 目标路径（支持 `~` 前缀；不存在仅警告，仍允许写入）
        path: String,
        /// 规则模式：deny（拒绝读写）/ readonly（仅可读）
        #[arg(long, default_value = "readonly")]
        mode: String,
    },
    /// 列出当前生效的全部 path_rules（builtin / user / session 三段）
    List,
}

#[derive(Subcommand, Debug)]
pub enum PluginSub {
    /// 列出已加载插件
    List,
    /// 从磁盘路径加载插件
    Load {
        /// 插件根目录路径或清单文件（plugin.json）路径
        path: String,
    },
    /// 卸载已加载的插件
    Unload {
        /// 插件 ID
        id: String,
    },
    /// 启用已加载的插件
    Enable {
        /// 插件 ID
        id: String,
    },
    /// 禁用已加载的插件
    Disable {
        /// 插件 ID
        id: String,
    },
    /// 查看插件详细信息
    Info {
        /// 插件 ID
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuditSub {
    /// 列出最近的审计记录
    List {
        /// 最多显示条数（默认 50）
        #[arg(short, long)]
        limit: Option<u32>,
    },
    /// 查看单条审计记录详情
    Show {
        /// 审计记录序号
        id: String,
    },
    /// 导出审计记录为 JSON 文件
    Export {
        /// 导出目标文件路径（JSON 格式）
        path: PathBuf,
    },
}

/// 解析参数并执行对应子命令；无子命令时默认执行 chat。
pub fn run_cli() -> Result<(), AppError> {
    let cli = Cli::parse();
    let cmd = cli.command.unwrap_or(Commands::Chat { resume: false });

    match cmd {
        Commands::Init => return run_init(),
        Commands::Doctor => return run_doctor(),
        _ => {}
    }

    let config_path = normalize_path(DEFAULT_CONFIG_PATH).ok();
    let cfg = load_config(config_path.as_deref())?;
    if let Err(e) = validate_config(&cfg) {
        eprintln!("配置不合法: {}", e);
        return Ok(());
    }
    ensure_work_dir_structure(&cfg)?;
    ensure_embedded_assets(&cfg)?;

    // 在 init_logging 之前加载 .env，使 RUST_LOG 等变量参与 EnvFilter（dotenvy 默认不覆盖已存在的环境变量）。
    if let Ok(work_dir) = get_work_dir(&cfg) {
        let _ = dotenvy::from_path(work_dir.join("assets").join(".env"));
    }

    let log_dir = resolve_log_dir(&cfg)?;
    std::fs::create_dir_all(&log_dir).map_err(AppError::Io)?;
    init_logging(
        &cfg.log,
        if cfg.log.file_enabled {
            Some(log_dir.as_path())
        } else {
            None
        },
    )?;

    match cmd {
        Commands::Config { sub } => run_config(sub, &cfg),
        Commands::Session { sub } => run_session(sub, &cfg),
        Commands::Workspace { sub } => run_workspace(sub, &cfg),
        Commands::Pathrules { sub } => run_pathrules(sub, &cfg),
        Commands::Plugin { sub } => run_plugin(sub, &cfg),
        Commands::Audit { sub } => run_audit(sub, &cfg),
        Commands::Chat { resume } => run_chat(resume, &cfg),
        _ => unreachable!(),
    }
}
