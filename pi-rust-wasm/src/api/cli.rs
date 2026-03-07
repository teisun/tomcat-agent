//! CLI 子命令：init、doctor、config、session、plugin、audit；无参默认 chat。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{load_config, normalize_path, validate_config, AppConfig, AppError, SessionManager};

const DEFAULT_CONFIG_PATH: &str = "~/.pi/agent/config.toml";

/// pi-awsm：会话存储与 CLI 子命令（init/doctor/config/session/plugin/audit）。
#[derive(Parser, Debug)]
#[command(name = "pi-awsm", about = "PI Agent CLI", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 初始化配置，引导 LLM 与安全策略，生成配置文件
    Init {
        /// 配置文件输出路径
        #[arg(short, long, default_value = DEFAULT_CONFIG_PATH)]
        config: String,
    },
    /// 检测运行环境、WasmEdge/QuickJS、配置合法性，输出修复建议
    Doctor {
        /// 配置文件路径
        #[arg(short, long)]
        config: Option<String>,
    },
    /// 配置管理：get/set/edit/export/import
    Config {
        #[command(subcommand)]
        sub: ConfigSub,
    },
    /// 会话管理：list/new/switch/delete/archive/search
    Session {
        #[command(subcommand)]
        sub: SessionSub,
    },
    /// 插件管理（依赖 T1-P0-009，当前占位）
    Plugin {
        #[command(subcommand)]
        sub: PluginSub,
    },
    /// 审计日志查看（P0 可占位或只读已有日志）
    Audit {
        #[command(subcommand)]
        sub: AuditSub,
    },
    /// 对话模式（由 chat 角色实现，此处仅占位）
    Chat,
}

#[derive(Subcommand, Debug)]
pub enum ConfigSub {
    /// 获取配置项
    Get { key: Option<String> },
    /// 设置配置项
    Set { key: String, value: String },
    /// 用编辑器打开配置文件
    Edit {
        #[arg(short, long)]
        config: Option<String>,
    },
    /// 导出配置到文件
    Export { path: PathBuf },
    /// 从文件导入配置
    Import { path: PathBuf },
}

#[derive(Subcommand, Debug)]
pub enum SessionSub {
    /// 列出所有会话
    List,
    /// 创建新会话
    New {
        #[arg(short, long)]
        cwd: Option<String>,
    },
    /// 切换到指定会话
    Switch { key: String },
    /// 删除会话
    Delete { key: String },
    /// 归档会话
    Archive { key: String },
    /// 搜索会话（MVP 占位：仅列出）
    Search { query: Option<String> },
}

#[derive(Subcommand, Debug)]
pub enum PluginSub {
    /// 列出已加载插件
    List,
    /// 加载插件
    Load { path: String },
    /// 卸载插件
    Unload { id: String },
    /// 启用插件
    Enable { id: String },
    /// 禁用插件
    Disable { id: String },
    /// 插件详情
    Info { id: String },
}

#[derive(Subcommand, Debug)]
pub enum AuditSub {
    /// 列出审计记录
    List { limit: Option<u32> },
    /// 查看单条
    Show { id: String },
    /// 导出
    Export { path: PathBuf },
}

/// 解析参数并执行对应子命令；无子命令时默认执行 chat。
pub fn run_cli() -> Result<(), AppError> {
    let cli = Cli::parse();
    let cmd = cli.command.unwrap_or(Commands::Chat);
    match cmd {
        Commands::Init { config } => run_init(&config),
        Commands::Doctor { config } => run_doctor(config.as_deref()),
        Commands::Config { sub } => run_config(sub),
        Commands::Session { sub } => run_session(sub),
        Commands::Plugin { sub } => run_plugin(sub),
        Commands::Audit { sub } => run_audit(sub),
        Commands::Chat => run_chat(),
    }
}

pub(crate) fn run_init(config_path: &str) -> Result<(), AppError> {
    let path = normalize_path(config_path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("无效配置路径".to_string()))?;
    std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    let cfg = AppConfig::default();
    let toml = toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(&path, toml).map_err(AppError::Io)?;
    println!("已生成配置文件: {}", path.display());
    println!("请编辑 {} 填写 LLM API 与安全策略。", path.display());
    Ok(())
}

pub(crate) fn run_doctor(config_path: Option<&str>) -> Result<(), AppError> {
    if config_path.is_none() {
        let default = normalize_path(DEFAULT_CONFIG_PATH)?;
        if !default.exists() {
            println!("未找到配置文件。请先运行: pi-awsm init");
            return Ok(());
        }
    }
    let path: Option<PathBuf> = config_path
        .map(|s| normalize_path(s).ok())
        .flatten()
        .or_else(|| normalize_path(DEFAULT_CONFIG_PATH).ok());
    let path = match path {
        Some(p) if p.exists() => p,
        _ => {
            println!("未找到配置文件。请先运行: pi-awsm init");
            return Ok(());
        }
    };
    let cfg = load_config(Some(path.as_path()))?;
    if let Err(e) = validate_config(&cfg) {
        println!("配置不合法: {}", e);
        return Ok(());
    }
    println!("配置合法。");
    // WasmEdge/QuickJS 可用性：占位，后续由 wasm_plugin 对接
    println!("WasmEdge/QuickJS 检测: 占位（待 T1-P0-009 完成后对接）");
    Ok(())
}

pub(crate) fn run_config(sub: ConfigSub) -> Result<(), AppError> {
    match sub {
        ConfigSub::Get { key } => {
            let cfg = load_config(None)?;
            if let Some(k) = key {
                println!("get key: {} (占位)", k);
            } else {
                let toml =
                    toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
                println!("{}", toml);
            }
        }
        ConfigSub::Set { key, value } => println!("set {} = {} (占位)", key, value),
        ConfigSub::Edit { config: _ } => println!("edit: 请手动编辑配置文件"),
        ConfigSub::Export { path } => {
            let cfg = load_config(None)?;
            let toml = toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
            std::fs::write(&path, toml).map_err(AppError::Io)?;
            println!("已导出到 {}", path.display());
        }
        ConfigSub::Import { path } => {
            let content = std::fs::read_to_string(&path).map_err(AppError::Io)?;
            let _: AppConfig =
                toml::from_str(&content).map_err(|e| AppError::Config(e.to_string()))?;
            println!(
                "已从 {} 导入（当前仅校验格式，未写入默认路径）",
                path.display()
            );
        }
    }
    Ok(())
}

pub(crate) fn run_session(sub: SessionSub) -> Result<(), AppError> {
    let cfg = load_config(None)?;
    let mgr = SessionManager::from_sessions_dir(&cfg.storage.sessions_dir)?;
    match sub {
        SessionSub::List => {
            let list = mgr.list_sessions()?;
            if list.is_empty() {
                println!("当前无会话。使用 session new 创建。");
                return Ok(());
            }
            for (key, entry) in list {
                println!("{}  {}  {}", key, entry.session_id, entry.updated_at);
            }
        }
        SessionSub::New { cwd } => {
            let key = mgr.current_session_key();
            let entry = mgr.create_session(key, cwd)?;
            println!("已创建会话: {}  {}", entry.session_id, key);
        }
        SessionSub::Switch { key } => {
            if mgr.get_session(&key)?.is_none() {
                println!("会话不存在: {}", key);
                return Ok(());
            }
            println!("当前会话 key 固定为 agent:default:main，切换逻辑占位。");
        }
        SessionSub::Delete { key } => {
            mgr.delete_session(&key)?;
            println!("已删除会话: {}", key);
        }
        SessionSub::Archive { key } => {
            mgr.archive_session(&key)?;
            println!("已归档会话: {}", key);
        }
        SessionSub::Search { query } => {
            let list = mgr.list_sessions()?;
            if list.is_empty() {
                println!("无会话");
                return Ok(());
            }
            let q = query.as_deref().unwrap_or("");
            for (key, entry) in list {
                if q.is_empty() || key.contains(q) || entry.session_id.contains(q) {
                    println!("{}  {}", key, entry.session_id);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn run_plugin(sub: PluginSub) -> Result<(), AppError> {
    match sub {
        PluginSub::List => println!("插件列表（占位，依赖 T1-P0-009）"),
        PluginSub::Load { path } => println!("load {}（占位）", path),
        PluginSub::Unload { id } => println!("unload {}（占位）", id),
        PluginSub::Enable { id } => println!("enable {}（占位）", id),
        PluginSub::Disable { id } => println!("disable {}（占位）", id),
        PluginSub::Info { id } => println!("info {}（占位）", id),
    }
    Ok(())
}

pub(crate) fn run_audit(sub: AuditSub) -> Result<(), AppError> {
    match sub {
        AuditSub::List { limit } => println!("审计列表（占位）limit={:?}", limit),
        AuditSub::Show { id } => println!("审计 show {}（占位）", id),
        AuditSub::Export { path } => println!("审计导出到 {}（占位）", path.display()),
    }
    Ok(())
}

pub(crate) fn run_chat() -> Result<(), AppError> {
    println!("对话模式由 chat 角色实现，当前为占位。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parse_init() {
        let cli = Cli::try_parse_from(&["pi-awsm", "init"]).unwrap();
        let cmd = cli.command.expect("subcommand");
        assert!(matches!(cmd, Commands::Init { config: _ }));
        if let Commands::Init { config } = cmd {
            assert!(config.contains("config.toml"));
        }
    }

    #[test]
    fn cli_parse_init_with_config_path() {
        let cli =
            Cli::try_parse_from(&["pi-awsm", "init", "--config", "/tmp/pi/config.toml"]).unwrap();
        let cmd = cli.command.unwrap();
        if let Commands::Init { config } = cmd {
            assert_eq!(config, "/tmp/pi/config.toml");
        }
    }

    #[test]
    fn cli_parse_doctor() {
        let cli = Cli::try_parse_from(&["pi-awsm", "doctor"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Doctor { config: None })
        ));
    }

    #[test]
    fn cli_parse_config_get() {
        let cli = Cli::try_parse_from(&["pi-awsm", "config", "get"]).unwrap();
        let cmd = cli.command.unwrap();
        if let Commands::Config { sub } = cmd {
            assert!(matches!(sub, ConfigSub::Get { key: None }));
        }
    }

    #[test]
    fn cli_parse_session_list() {
        let cli = Cli::try_parse_from(&["pi-awsm", "session", "list"]).unwrap();
        let cmd = cli.command.unwrap();
        assert!(matches!(
            cmd,
            Commands::Session {
                sub: SessionSub::List
            }
        ));
    }

    #[test]
    fn cli_parse_plugin_list() {
        let cli = Cli::try_parse_from(&["pi-awsm", "plugin", "list"]).unwrap();
        let cmd = cli.command.unwrap();
        assert!(matches!(
            cmd,
            Commands::Plugin {
                sub: PluginSub::List
            }
        ));
    }

    #[test]
    fn cli_parse_audit_list() {
        let cli = Cli::try_parse_from(&["pi-awsm", "audit", "list"]).unwrap();
        let cmd = cli.command.unwrap();
        assert!(matches!(
            cmd,
            Commands::Audit {
                sub: AuditSub::List { limit: None }
            }
        ));
    }

    #[test]
    fn cli_parse_default_chat() {
        let cli = Cli::try_parse_from(&["pi-awsm"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn run_init_creates_config_in_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let r = run_init(config_path.to_str().unwrap());
        assert!(r.is_ok());
        assert!(config_path.exists());
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("[log]") || content.contains("log"));
    }

    #[test]
    fn run_doctor_none_when_no_default_config_returns_ok() {
        let r = run_doctor(None);
        assert!(r.is_ok());
    }

    #[test]
    fn run_doctor_some_with_valid_config_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        run_init(config_path.to_str().unwrap()).unwrap();
        let r = run_doctor(Some(config_path.to_str().unwrap()));
        assert!(r.is_ok());
    }

    #[test]
    fn run_plugin_list_returns_ok() {
        let r = run_plugin(PluginSub::List);
        assert!(r.is_ok());
    }

    #[test]
    fn run_audit_list_returns_ok() {
        let r = run_audit(AuditSub::List { limit: None });
        assert!(r.is_ok());
    }

    #[test]
    fn run_chat_returns_ok() {
        let r = run_chat();
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_get_with_key_returns_ok() {
        let r = run_config(ConfigSub::Get {
            key: Some("log.level".to_string()),
        });
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_get_without_key_returns_ok() {
        let r = run_config(ConfigSub::Get { key: None });
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_export_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.toml");
        let r = run_config(ConfigSub::Export {
            path: out.clone(),
        });
        assert!(r.is_ok());
        assert!(out.exists());
    }

    #[test]
    fn run_config_import_valid_toml_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("import.toml");
        let toml = toml::to_string_pretty(&AppConfig::default()).unwrap();
        std::fs::write(&path, toml).unwrap();
        let r = run_config(ConfigSub::Import { path });
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_set_returns_ok() {
        let r = run_config(ConfigSub::Set {
            key: "log.level".to_string(),
            value: "debug".to_string(),
        });
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_edit_returns_ok() {
        let r = run_config(ConfigSub::Edit { config: None });
        assert!(r.is_ok());
    }

    #[test]
    fn run_doctor_invalid_config_path_returns_ok() {
        let r = run_doctor(Some("/nonexistent/path/config.toml"));
        assert!(r.is_ok());
    }

    fn sessions_dir_from_temp(dir: &tempfile::TempDir) -> String {
        let path = dir.path().canonicalize().unwrap_or_else(|_| dir.path().to_path_buf());
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn run_session_list_empty_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::List);
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_new_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::New { cwd: None });
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_list_after_new_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let _ = run_session(SessionSub::New { cwd: None });
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::List);
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_switch_nonexistent_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::Switch {
            key: "nonexistent".to_string(),
        });
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_switch_existing_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let _ = run_session(SessionSub::New { cwd: None });
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::Switch {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        });
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_delete_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let _ = run_session(SessionSub::New { cwd: None });
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::Delete {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        });
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok(), "run_session(Delete) failed: {:?}", r);
    }

    #[test]
    fn run_session_archive_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let _ = run_session(SessionSub::New { cwd: None });
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::Archive {
            key: crate::DEFAULT_SESSION_KEY.to_string(),
        });
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_search_empty_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::Search { query: None });
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_search_with_query_returns_ok() {
        let _dir = tempfile::tempdir().unwrap();
        let sessions_dir = sessions_dir_from_temp(&_dir);
        std::env::set_var("PI_AWSM__STORAGE__SESSIONS_DIR", &sessions_dir);
        let r = run_session(SessionSub::Search {
            query: Some("q".to_string()),
        });
        std::env::remove_var("PI_AWSM__STORAGE__SESSIONS_DIR");
        assert!(r.is_ok());
    }

    #[test]
    fn run_audit_show_and_export_returns_ok() {
        let r = run_audit(AuditSub::Show {
            id: "id1".to_string(),
        });
        assert!(r.is_ok());
        let dir = tempfile::tempdir().unwrap();
        let r2 = run_audit(AuditSub::Export {
            path: dir.path().join("audit.json"),
        });
        assert!(r2.is_ok());
    }
}
