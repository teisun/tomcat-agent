//! CLI 子命令：init、doctor、config、session、plugin、audit；无参默认按配置进入 claw/code。

mod audit_cmd;
mod builtin_plugins;
mod chat_cmd;
mod claw_cmd;
mod code_cmd;
mod config_cmd;
mod init;
pub(crate) mod init_model_wizard;
mod model_cmd;
mod models_toml;
mod package_cmd;
mod pathrules_cmd;
mod plugin_cmd;
mod session_cmd;
mod skill_cmd;
pub(crate) mod splash;
mod workspace_cmd;

#[cfg(test)]
mod tests;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, init_logging, load_config,
    normalize_path, resolve_log_dir, validate_config, AppConfig, AppError, CLI_NAME,
    DEFAULT_CONFIG_PATH,
};

pub(crate) use audit_cmd::run_audit;
pub use chat_cmd::build_runtime_and_context;
pub use chat_cmd::build_runtime_and_context_with_overrides;
pub(crate) use claw_cmd::run_claw;
pub(crate) use code_cmd::run_code;
pub(crate) use config_cmd::{config_file_path, run_config};
pub(crate) use init::{run_doctor, run_init};
pub(crate) use model_cmd::run_model;
pub(crate) use package_cmd::{run_install, run_packages, run_uninstall};
pub(crate) use pathrules_cmd::run_pathrules;
pub(crate) use plugin_cmd::run_plugin;
pub(crate) use session_cmd::run_session;
pub(crate) use skill_cmd::run_skill;
pub(crate) use workspace_cmd::run_workspace;

#[cfg(test)]
pub(crate) use audit_cmd::{parse_audit_line, read_audit_entries};
#[cfg(test)]
pub(crate) use config_cmd::{resolve_toml_key, set_toml_key};
#[cfg(test)]
pub(crate) use init::{
    auto_add_to_path, install_canonical_symlink, path_export_targets, prune_stale_lines,
};
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
    long_about = "tomcat 是基于 rquickjs 的插件化 AI Agent 运行时。\n支持 init/doctor/config/session/plugin/audit 子命令；`tomcat claw` 提供全局会话，`tomcat code` 提供按项目隔离的对话模式。",
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
    /// 检测运行环境、QuickJS 资源与配置合法性，输出修复建议
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
    /// 插件管理：list/load/build/unload/enable/disable/info
    Plugin {
        #[command(subcommand)]
        sub: PluginSub,
    },
    /// 审计日志：list/show/export
    Audit {
        #[command(subcommand)]
        sub: AuditSub,
    },
    /// Skill 管理：list/reload
    Skill {
        #[command(subcommand)]
        sub: SkillSub,
    },
    /// 模型管理：列出 / 新增 / 删除 / Key / 默认模型
    Model {
        #[command(subcommand)]
        sub: ModelSub,
    },
    /// 安装 package / bare plugin / bare skill
    Install {
        /// 本地 source 路径
        source: String,
        /// 安装可见层
        #[arg(long, value_enum)]
        visibility: Option<PackageVisibilityArg>,
        /// scope 安装时显式指定 project 根目录
        #[arg(long)]
        scope_root: Option<String>,
        /// 允许覆盖当前层已有同名资源
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// 卸载某层 package 账本记录及其资源目录
    Uninstall {
        /// package 名称
        package: String,
        /// 要卸载的目标层
        #[arg(long, value_enum)]
        visibility: Option<PackageVisibilityArg>,
        /// scope 卸载时显式指定 project 根目录
        #[arg(long)]
        scope_root: Option<String>,
    },
    /// 列出各层已安装 packages
    Packages {
        /// 只查看某一层；缺省时展示当前 scope + agent + global
        #[arg(long, value_enum)]
        visibility: Option<PackageVisibilityArg>,
        /// scope 视图显式指定 project 根目录
        #[arg(long)]
        scope_root: Option<String>,
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
    /// 进入全局会话模式（不绑定 cwd）
    Claw {
        /// 恢复上次会话（默认行为，显式语义）
        #[arg(long, default_value_t = false)]
        resume: bool,
    },
    /// 进入按项目隔离的对话模式
    Code {
        /// 恢复上次会话（默认行为，显式语义）
        #[arg(long, default_value_t = false)]
        resume: bool,
    },
    /// 以 stdio 协议暴露多会话 Agent Server 给 IDE / GUI
    Serve {
        /// 显式选择 stdio 传输（Phase 1 主路径）
        #[arg(long, default_value_t = false, conflicts_with = "ws")]
        stdio: bool,
        /// 预留给 Phase 2 的 WebSocket 传输
        #[arg(long, default_value_t = false, conflicts_with = "stdio")]
        ws: bool,
        /// 导出 serve 协议 schema 工件并退出
        #[arg(long = "print-schema", default_value_t = false)]
        print_schema: bool,
    },
    /// 兼容旧命令；等价于 `tomcat code`
    #[command(hide = true)]
    Chat {
        /// 恢复上次会话（默认行为，显式语义）
        #[arg(long, default_value_t = false)]
        resume: bool,
    },
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScopeArg {
    Code,
    Claw,
}

impl SessionScopeArg {
    pub fn into_mode(self) -> crate::SessionMode {
        match self {
            Self::Code => crate::SessionMode::Code,
            Self::Claw => crate::SessionMode::Claw,
        }
    }
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageVisibilityArg {
    Scope,
    Agent,
    Global,
}

impl PackageVisibilityArg {
    pub fn into_visibility(self) -> crate::core::package::PackageVisibility {
        match self {
            Self::Scope => crate::core::package::PackageVisibility::Scope,
            Self::Agent => crate::core::package::PackageVisibility::Agent,
            Self::Global => crate::core::package::PackageVisibility::Global,
        }
    }
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
    List {
        #[arg(long, value_enum)]
        scope: Option<SessionScopeArg>,
    },
    /// 创建新会话
    New {
        #[arg(long, value_enum)]
        scope: Option<SessionScopeArg>,
    },
    /// 切换到指定 session_id
    Switch {
        session_id: String,
        #[arg(long, value_enum)]
        scope: Option<SessionScopeArg>,
    },
    /// 删除会话
    Delete {
        session_id: String,
        #[arg(long, value_enum)]
        scope: Option<SessionScopeArg>,
    },
    /// 归档会话
    Archive {
        session_id: String,
        #[arg(long, value_enum)]
        scope: Option<SessionScopeArg>,
    },
    /// 搜索会话（MVP 占位：仅列出）
    Search {
        query: Option<String>,
        #[arg(long, value_enum)]
        scope: Option<SessionScopeArg>,
    },
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
    /// 从 src/ 构建插件交付产物
    Build {
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

#[derive(Subcommand, Debug)]
pub enum SkillSub {
    /// 列出当前发现到的 skill 与诊断
    List,
    /// 重扫技能目录并打印新的发现结果
    Reload,
}

#[derive(Subcommand, Debug)]
pub enum ModelSub {
    /// 列出当前可见模型与 Key 状态
    List,
    /// 新增或覆盖一个用户模型
    Add {
        /// 模型 id（如 my-gateway）
        id: String,
        /// 上游协议族：openai / openai-responses / anthropic-messages
        #[arg(long)]
        api: String,
        /// provider 名（决定默认 API Key 环境变量名）
        #[arg(long)]
        provider: String,
        /// 实际请求发给上游的 model 名；缺省时等于 id
        #[arg(long)]
        model_name: Option<String>,
        /// API Key 环境变量名（默认按 provider 推断）
        #[arg(long)]
        api_key_env: Option<String>,
        /// 上游 base_url
        #[arg(long)]
        base_url: Option<String>,
        /// 是否支持 vision
        #[arg(long, default_value_t = false)]
        vision: bool,
        /// 是否支持 files
        #[arg(long, default_value_t = false)]
        files: bool,
        /// 是否支持 tools
        #[arg(long, default_value_t = true)]
        tools: bool,
        /// 是否支持 reasoning
        #[arg(long, default_value_t = false)]
        reasoning: bool,
        /// 是否支持 web search
        #[arg(long, default_value_t = false)]
        web_search: bool,
        /// context window
        #[arg(long)]
        context_window: Option<u32>,
        /// thinking format（如 deepseek / doubao / anthropic）
        #[arg(long)]
        thinking_format: Option<String>,
    },
    /// 删除一个用户模型；内置模型不可删
    Remove { id: String },
    /// 管理 provider API Key
    Key {
        #[command(subcommand)]
        sub: ModelKeySub,
    },
    /// 设置 llm.default_model
    Default { model: String },
}

#[derive(Subcommand, Debug)]
pub enum ModelKeySub {
    /// 写入 provider / ENV_NAME 对应的 API Key
    Set {
        provider: String,
        value: Option<String>,
    },
    /// 列出当前模型所需的 API Key 槽位
    List,
}

const TOMCAT_AGENT_ACTIVE_ENV: &str = "TOMCAT_AGENT_ACTIVE";
const NESTED_INVOCATION_REFUSAL: &str = "Refusing to run this Tomcat command inside an active Tomcat agent session because it would mutate session or global state. Use the agent's tool calls instead, or run the command from a separate terminal outside the active session.";

fn nested_agent_invocation_active() -> bool {
    matches!(std::env::var(TOMCAT_AGENT_ACTIVE_ENV).as_deref(), Ok("1"))
}

fn nested_invocation_mutates_state(cmd: &Commands) -> bool {
    match cmd {
        Commands::Init => true,
        Commands::Doctor => false,
        Commands::Config { sub } => !matches!(sub, ConfigSub::Get { .. }),
        Commands::Session { sub } => matches!(
            sub,
            SessionSub::New { .. }
                | SessionSub::Switch { .. }
                | SessionSub::Delete { .. }
                | SessionSub::Archive { .. }
        ),
        Commands::Plugin { sub } => matches!(
            sub,
            PluginSub::Load { .. }
                | PluginSub::Unload { .. }
                | PluginSub::Enable { .. }
                | PluginSub::Disable { .. }
        ),
        Commands::Audit { .. } => false,
        Commands::Skill { .. } => false,
        Commands::Model { sub } => !matches!(
            sub,
            ModelSub::List
                | ModelSub::Key {
                    sub: ModelKeySub::List
                }
        ),
        Commands::Install { .. } | Commands::Uninstall { .. } => true,
        Commands::Packages { .. } => false,
        Commands::Workspace { sub } => {
            matches!(sub, WorkspaceSub::Add { .. } | WorkspaceSub::Remove { .. })
        }
        Commands::Pathrules { sub } => matches!(sub, PathRulesSub::Add { .. }),
        Commands::Claw { .. }
        | Commands::Code { .. }
        | Commands::Serve { .. }
        | Commands::Chat { .. } => true,
    }
}

fn guard_nested_invocation(cmd: Option<&Commands>) -> Result<(), AppError> {
    if !nested_agent_invocation_active() {
        return Ok(());
    }
    let Some(cmd) = cmd else {
        return Ok(());
    };
    if nested_invocation_mutates_state(cmd) {
        return Err(AppError::Config(NESTED_INVOCATION_REFUSAL.to_string()));
    }
    Ok(())
}

/// 解析参数并执行对应子命令；无子命令时按配置进入默认 session mode。
pub fn run_cli() -> Result<(), AppError> {
    let cli = Cli::parse();
    let had_explicit_command = cli.command.is_some();

    guard_nested_invocation(cli.command.as_ref())?;

    match cli.command.as_ref() {
        Some(Commands::Init) => return run_init(),
        Some(Commands::Doctor) => return run_doctor(),
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
        let env_path = work_dir.join("assets").join(".env");
        let _ = dotenvy::from_path(&env_path);
        let _ = crate::core::llm::auth::refresh_managed_credentials(&env_path);
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

    let cmd = match cli.command {
        Some(cmd) => cmd,
        None => match resolve_default_cli_session_mode(&cfg)? {
            crate::SessionMode::Code => Commands::Code { resume: false },
            crate::SessionMode::Claw => Commands::Claw { resume: false },
        },
    };

    if !had_explicit_command {
        guard_nested_invocation(Some(&cmd))?;
    }

    match cmd {
        Commands::Init => unreachable!("init handled before config load"),
        Commands::Doctor => unreachable!("doctor handled before config load"),
        Commands::Config { sub } => run_config(sub, &cfg),
        Commands::Session { sub } => run_session(sub, &cfg),
        Commands::Install {
            source,
            visibility,
            scope_root,
            force,
        } => run_install(source, visibility, scope_root, force, &cfg),
        Commands::Uninstall {
            package,
            visibility,
            scope_root,
        } => run_uninstall(package, visibility, scope_root, &cfg),
        Commands::Packages {
            visibility,
            scope_root,
        } => run_packages(visibility, scope_root, &cfg),
        Commands::Workspace { sub } => run_workspace(sub, &cfg),
        Commands::Pathrules { sub } => run_pathrules(sub, &cfg),
        Commands::Plugin { sub } => run_plugin(sub, &cfg),
        Commands::Audit { sub } => run_audit(sub, &cfg),
        Commands::Skill { sub } => run_skill(sub, &cfg),
        Commands::Model { sub } => run_model(sub, &cfg),
        Commands::Claw { resume } => run_claw(resume, &cfg),
        Commands::Code { resume } => run_code(resume, &cfg),
        Commands::Serve {
            stdio,
            ws,
            print_schema,
        } => crate::api::serve::run_serve(
            crate::api::serve::ServeCliArgs {
                stdio,
                ws,
                print_schema,
            },
            &cfg,
        ),
        Commands::Chat { resume } => run_code(resume, &cfg),
    }
}

pub(crate) fn resolve_default_cli_session_mode(
    cfg: &AppConfig,
) -> Result<crate::SessionMode, AppError> {
    let env_override = std::env::var("TOMCAT_SESSION_MODE").ok();
    crate::resolve_session_mode(&cfg.session.default_mode, env_override.as_deref())
}
