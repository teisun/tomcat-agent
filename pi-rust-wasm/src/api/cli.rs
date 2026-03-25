//! CLI 子命令：init、doctor、config、session、plugin、audit；无参默认 chat。

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, load_config, normalize_path,
    resolve_agent_dir, resolve_audit_dir, resolve_plugins_dir, resolve_quickjs_path,
    resolve_sessions_dir, validate_config, wire, write_file_atomic, AppConfig, AppError,
    AuditFilter, AuditStore, DefaultEventBus, DefaultToolRegistry, EventBus, FileAuditRecorder,
    PluginManager, SessionManager, Tool, ToolExecutor, ToolRegistry, TracingAuditRecorder,
    WasmEngine, WasmEngineConfig, DEFAULT_LLM_MODEL,
};

const DEFAULT_CONFIG_PATH: &str = "~/.pi_/pi.config.toml";

/// pi CLI：AI Agent 运行时，支持插件管理、会话、配置、审计与对话模式
#[derive(Parser, Debug)]
#[command(
    name = "pi",
    about = "PI Agent CLI — 插件化 AI Agent 运行时",
    long_about = "pi 是基于 WasmEdge + QuickJS 的插件化 AI Agent 运行时。\n支持 init/doctor/config/session/plugin/audit 子命令，无参数时进入对话模式。",
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

    if let Ok(work_dir) = get_work_dir(&cfg) {
        let _ = dotenvy::from_path(work_dir.join("assets").join(".env"));
    }

    match cmd {
        Commands::Config { sub } => run_config(sub, &cfg),
        Commands::Session { sub } => run_session(sub, &cfg),
        Commands::Workspace { sub } => run_workspace(sub, &cfg),
        Commands::Plugin { sub } => run_plugin(sub, &cfg),
        Commands::Audit { sub } => run_audit(sub, &cfg),
        Commands::Chat { resume } => run_chat(resume, &cfg),
        _ => unreachable!(),
    }
}

pub(crate) fn run_init() -> Result<(), AppError> {
    let config_file = normalize_path(DEFAULT_CONFIG_PATH)?;

    // --- [1/3] 环境初始化（标题先于配置写入，便于失败时仍可见步骤）---
    println!("\n[1/3] 环境初始化");

    // --- 幂等性：配置文件已存在则默认不覆盖 ---
    let mut write_config = true;
    if config_file.exists() {
        write_config = false;
        println!("  已存在配置文件，保留现有内容：{}", config_file.display());
    }

    let cfg = if write_config {
        let llm = crate::LlmConfig {
            provider: "openai".to_string(),
            default_model: DEFAULT_LLM_MODEL.to_string(),
            api_base: None,
            ..Default::default()
        };
        AppConfig {
            llm,
            ..Default::default()
        }
    } else {
        crate::load_config(Some(&config_file)).unwrap_or_default()
    };

    if write_config {
        if let Some(parent) = config_file.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        let toml_str = toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
        std::fs::write(&config_file, toml_str).map_err(AppError::Io)?;
    }

    if write_config {
        println!("  ✓ 配置文件已写入: {}", config_file.display());
    } else {
        println!("  ✓ 使用已有配置文件: {}", config_file.display());
    }
    println!("  ✓ 默认 LLM Provider: {}", cfg.llm.provider);
    println!("  ✓ 默认模型: {}", cfg.llm.default_model);

    ensure_work_dir_structure(&cfg)?;
    println!("  ✓ 目录结构就绪");

    ensure_embedded_assets(&cfg)?;
    println!("  ✓ 内嵌资源已释放（wasm + modules）");

    match std::env::current_exe() {
        Ok(exe) => {
            if let Some(bin_dir) = exe.parent() {
                if auto_add_to_path(bin_dir) {
                    println!("  ✓ 已加入 PATH 环境变量");
                } else {
                    println!("  ⚠ 无法自动配置 PATH，请手动执行：");
                    println!("    export PATH=\"{}:$PATH\"", bin_dir.display());
                }
            } else {
                println!("  ⚠ 无法确定可执行文件所在目录，请手动配置 PATH");
            }
        }
        Err(_) => println!("  ⚠ 无法确定可执行文件路径，请手动配置 PATH"),
    }

    // --- [2/3] 资源检查（与 pi doctor 一致，跳过 API Key）---
    println!("\n[2/3] 资源检查");
    run_doctor_checks(&cfg, config_file.as_path(), true)?;

    // --- [3/3] API Key 配置 ---
    println!("\n[3/3] API Key 配置");
    let work_dir = get_work_dir(&cfg)?;
    let env_path = work_dir.join("assets").join(".env");
    let existing_key = env_path
        .exists()
        .then(|| {
            dotenvy::from_path_iter(&env_path)
                .ok()
                .and_then(|iter| {
                    iter.filter_map(|r| r.ok())
                        .find(|(k, _)| k == "OPENAI_API_KEY")
                        .map(|(_, v)| v)
                })
                .filter(|v| !v.is_empty())
        })
        .flatten();

    if existing_key.is_some() {
        println!("  ✓ API Key 已配置");
    } else {
        let api_key: String = dialoguer::Password::new()
            .with_prompt("  输入 OPENAI_API_KEY（回车跳过）")
            .allow_empty_password(true)
            .interact()
            .unwrap_or_default();

        if api_key.is_empty() {
            println!(
                "  ⚠ API Key 未设置，后续可运行 `pi init` 重新配置，或编辑 {}",
                env_path.display()
            );
        } else {
            let env_content = format!(
                "# pi runtime credentials — 此文件由 pi init 生成，权限 0600\n\
                 OPENAI_API_KEY={api_key}\n\
                 \n\
                 # 如需通过代理访问 OpenAI，取消以下注释并填入代理地址：\n\
                 # HTTPS_PROXY=http://127.0.0.1:7890\n\
                 # HTTP_PROXY=http://127.0.0.1:7890\n\
                 # ALL_PROXY=socks5://127.0.0.1:7890\n"
            );
            std::fs::write(&env_path, env_content).map_err(AppError::Io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                std::fs::set_permissions(&env_path, perms).map_err(AppError::Io)?;
            }
            println!("  ✓ API Key 已写入 .env");
        }
    }

    println!("\n初始化完成！运行 `pi chat` 开始对话。");

    Ok(())
}

/// 将 `pi` 所在目录追加到 shell 启动脚本中的 PATH；已存在同序 export 则跳过。
fn auto_add_to_path(bin_dir: &Path) -> bool {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let profile = if shell.contains("zsh") {
        home.join(".zshrc")
    } else if shell.contains("bash") {
        let bp = home.join(".bash_profile");
        if bp.exists() {
            bp
        } else {
            home.join(".bashrc")
        }
    } else {
        home.join(".profile")
    };
    let export_line = format!("export PATH=\"{}:$PATH\"", bin_dir.display());
    if let Ok(content) = std::fs::read_to_string(&profile) {
        if content.contains(&export_line) {
            return true;
        }
    }
    let mut f = match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&profile)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    writeln!(f, "\n# Added by pi init\n{}", export_line).is_ok()
}

/// 与 `pi doctor` 相同的逐项检查。`skip_api_key` 为 true 时（用于 `pi init` 第二步）不输出 .env 权限与 OPENAI_API_KEY 相关行。
pub(crate) fn run_doctor_checks(
    cfg: &AppConfig,
    config_path: &Path,
    skip_api_key: bool,
) -> Result<(), AppError> {
    if let Err(e) = validate_config(cfg) {
        println!("✗ 配置不合法: {}", e);
        println!(
            "  → 运行 pi init 重新生成或手动修复 {}",
            config_path.display()
        );
        return Ok(());
    }
    if let Err(e) = ensure_work_dir_structure(cfg) {
        println!("✗ 创建工作目录失败: {}", e);
        return Ok(());
    }
    println!("✓ 配置合法 ({})", config_path.display());

    // --- 内嵌资源 ---
    if let Err(e) = ensure_embedded_assets(cfg) {
        println!("✗ 资源释放失败: {}", e);
        println!("  → 运行 pi init 或检查磁盘空间");
    } else {
        println!("✓ 内嵌资源已就绪");
    }

    // --- QuickJS wasm ---
    let resolved_qjs = resolve_quickjs_path(cfg);
    match &resolved_qjs {
        Some(p) => println!("✓ QuickJS wasm：{}", p.display()),
        None => {
            println!("✗ QuickJS wasm 未找到");
            println!("  → 运行 pi init 释放内嵌资源");
        }
    }

    // --- WasmEdge 运行时 ---
    let wasm_cfg = WasmEngineConfig {
        quickjs_path: resolved_qjs
            .as_ref()
            .and_then(|p| p.to_str())
            .map(String::from),
        ..Default::default()
    };
    match WasmEngine::global(Some(wasm_cfg)) {
        Ok(_) => println!("✓ WasmEdge 运行时：可用"),
        Err(e) => {
            println!("✗ WasmEdge 运行时：不可用 ({})", e);
            println!("  → 安装 WasmEdge: https://wasmedge.org/docs/start/install");
        }
    }

    // --- .versions.json SHA-256 ---
    let work_dir = get_work_dir(cfg)?;
    let versions_path = work_dir.join("assets").join(".versions.json");
    if versions_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&versions_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                let wasm_sha = v["wasm_sha256"].as_str().unwrap_or("N/A");
                let modules_sha = v["modules_sha256"].as_str().unwrap_or("N/A");
                println!(
                    "  资源版本: wasm={:.12}… modules={:.12}…",
                    wasm_sha, modules_sha
                );
            }
        }
    }

    if !skip_api_key {
        // --- .env 检查 ---
        let env_path = work_dir.join("assets").join(".env");
        if env_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&env_path) {
                    let mode = meta.permissions().mode() & 0o777;
                    if mode == 0o600 {
                        println!("✓ .env 权限: 0600");
                    } else {
                        println!("⚠ .env 权限: {:04o}（建议 0600）", mode);
                        println!("  → chmod 600 {}", env_path.display());
                    }
                }
            }
            #[cfg(not(unix))]
            println!("✓ .env 存在");
        } else {
            println!("⚠ .env 不存在（API Key 未配置）");
            println!("  → 运行 pi init 配置 API Key");
        }

        // --- OPENAI_API_KEY ---
        match std::env::var("OPENAI_API_KEY") {
            Ok(k) if !k.is_empty() => println!("✓ OPENAI_API_KEY 已设置"),
            _ => {
                println!("⚠ OPENAI_API_KEY 未设置");
                println!("  → 运行 pi init 或编辑 {}", env_path.display());
            }
        }
    }

    Ok(())
}

pub(crate) fn run_doctor() -> Result<(), AppError> {
    let path = match normalize_path(DEFAULT_CONFIG_PATH) {
        Ok(p) if p.exists() => p,
        _ => {
            println!("✗ 未找到配置文件");
            println!("  → 运行 pi init 生成配置");
            return Ok(());
        }
    };
    let cfg = load_config(Some(path.as_path()))?;
    run_doctor_checks(&cfg, path.as_path(), false)?;
    Ok(())
}

fn config_file_path() -> Result<PathBuf, AppError> {
    normalize_path(DEFAULT_CONFIG_PATH)
}

fn resolve_toml_key<'a>(val: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut current = val;
    for seg in key.split('.') {
        current = current.get(seg)?;
    }
    Some(current)
}

fn set_toml_key(val: &mut toml::Value, key: &str, raw_value: &str) -> Result<(), AppError> {
    let segments: Vec<&str> = key.split('.').collect();
    if segments.is_empty() {
        return Err(AppError::Config("配置项路径不能为空".to_string()));
    }

    let mut current = val;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            let table = current
                .as_table_mut()
                .ok_or_else(|| AppError::Config(format!("配置路径无效: {} 不是表", seg)))?;
            let entry = table.get(seg.to_owned()).ok_or_else(|| {
                let available: Vec<&String> = table.keys().collect();
                AppError::Config(format!(
                    "配置项不存在: {}。同级可用项: {:?}",
                    seg, available
                ))
            })?;
            let new_val =
                match entry {
                    toml::Value::Integer(_) => raw_value
                        .parse::<i64>()
                        .map(toml::Value::Integer)
                        .map_err(|_| {
                        AppError::Config(format!("无法将 '{}' 转换为整数类型", raw_value))
                    })?,
                    toml::Value::Boolean(_) => raw_value
                        .parse::<bool>()
                        .map(toml::Value::Boolean)
                        .map_err(|_| {
                            AppError::Config(format!(
                                "无法将 '{}' 转换为布尔类型（期望 true/false）",
                                raw_value
                            ))
                        })?,
                    toml::Value::Float(_) => raw_value
                        .parse::<f64>()
                        .map(toml::Value::Float)
                        .map_err(|_| {
                            AppError::Config(format!("无法将 '{}' 转换为浮点类型", raw_value))
                        })?,
                    _ => toml::Value::String(raw_value.to_string()),
                };
            table.insert(seg.to_string(), new_val);
            return Ok(());
        }
        current = current
            .get_mut(*seg)
            .ok_or_else(|| AppError::Config(format!("配置路径无效: 不存在的中间节点 {}", seg)))?;
    }
    Ok(())
}

pub(crate) fn run_config(sub: ConfigSub, cfg: &AppConfig) -> Result<(), AppError> {
    match sub {
        ConfigSub::Get { key } => {
            if let Some(k) = key {
                let val =
                    toml::Value::try_from(cfg).map_err(|e| AppError::Config(e.to_string()))?;
                match resolve_toml_key(&val, &k) {
                    Some(v) => println!("{}", v),
                    None => {
                        let parent_key = k.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
                        let parent = if parent_key.is_empty() {
                            Some(&val)
                        } else {
                            resolve_toml_key(&val, parent_key)
                        };
                        let hint = parent
                            .and_then(|p| p.as_table())
                            .map(|t| {
                                let keys: Vec<&String> = t.keys().collect();
                                format!("同级可用项: {:?}", keys)
                            })
                            .unwrap_or_default();
                        println!("未找到配置项: {}", k);
                        if !hint.is_empty() {
                            println!("  {}", hint);
                        }
                    }
                }
            } else {
                let toml_str =
                    toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
                println!("{}", toml_str);
            }
        }
        ConfigSub::Set { key, value } => {
            let path = config_file_path()?;
            if !path.exists() {
                println!("配置文件不存在: {}。请先运行: pi init", path.display());
                return Ok(());
            }
            let content = std::fs::read_to_string(&path).map_err(AppError::Io)?;
            let mut val: toml::Value = content
                .parse()
                .map_err(|e: toml::de::Error| AppError::Config(e.to_string()))?;
            set_toml_key(&mut val, &key, &value)?;
            let new_toml =
                toml::to_string_pretty(&val).map_err(|e| AppError::Config(e.to_string()))?;
            let check: Result<AppConfig, _> = toml::from_str(&new_toml);
            match check {
                Ok(ref c) => {
                    if let Err(e) = validate_config(c) {
                        println!("值无效: {}，未修改配置", e);
                        return Ok(());
                    }
                }
                Err(e) => {
                    println!("值无效: {}，未修改配置", e);
                    return Ok(());
                }
            }
            write_file_atomic(&path, new_toml.as_bytes())?;
            println!("已设置 {} = {}", key, value);
        }
        ConfigSub::Edit => {
            let path = config_file_path()?;
            if !path.exists() {
                println!("配置文件不存在: {}。请先运行: pi init", path.display());
                return Ok(());
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
                if cfg!(target_os = "windows") {
                    "notepad".to_string()
                } else {
                    "vi".to_string()
                }
            });
            match std::process::Command::new(&editor).arg(&path).status() {
                Ok(status) if status.success() => match load_config(Some(path.as_path())) {
                    Ok(ref c) => {
                        if let Err(e) = validate_config(c) {
                            println!("警告：编辑后的配置不合法: {}，请重新编辑修复", e);
                        } else {
                            println!("配置已更新");
                        }
                    }
                    Err(e) => {
                        println!("警告：编辑后的配置解析失败: {}，请重新编辑修复", e);
                    }
                },
                Ok(status) => {
                    println!("编辑器退出码: {}，配置可能未修改", status);
                }
                Err(e) => {
                    println!(
                        "无法启动编辑器 '{}': {}。请设置 EDITOR 环境变量或手动编辑 {}",
                        editor,
                        e,
                        path.display()
                    );
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn run_session(sub: SessionSub, cfg: &AppConfig) -> Result<(), AppError> {
    let sessions_path = resolve_sessions_dir(cfg)?;
    std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
    let mgr = SessionManager::new(sessions_path);
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
        SessionSub::New => {
            let key = mgr.current_session_key();
            let entry = mgr.create_session(key, None)?;
            println!("已创建会话: {}  {}", entry.session_id, key);
        }
        SessionSub::Switch { key } => {
            if mgr.get_session(&key)?.is_none() {
                println!("会话不存在: {}", key);
                return Ok(());
            }
            println!("当前会话 key 固定为 agent:main:main，切换逻辑占位。");
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

pub(crate) fn run_workspace(sub: WorkspaceSub, cfg: &AppConfig) -> Result<(), AppError> {
    let agent_dir = resolve_agent_dir(cfg)?;
    std::fs::create_dir_all(&agent_dir).map_err(AppError::Io)?;
    let ws_file = agent_dir.join("ext_workspaces.json");

    match sub {
        WorkspaceSub::Add { path, cwd } => {
            let target = if cwd {
                std::env::current_dir()
                    .map_err(|e| AppError::Config(format!("无法获取当前工作目录: {}", e)))?
            } else if let Some(p) = path {
                PathBuf::from(p)
            } else {
                return Err(AppError::Config("请提供目录路径或使用 --cwd".to_string()));
            };
            let abs = std::fs::canonicalize(&target).map_err(|_| {
                AppError::Config(format!("路径不存在或无法访问: {}", target.display()))
            })?;
            if !abs.is_dir() {
                return Err(AppError::Config(format!("路径不是目录: {}", abs.display())));
            }
            let mut workspaces = load_workspaces(&ws_file);
            if workspaces.contains(&abs) {
                println!("工作区已存在: {}", abs.display());
                return Ok(());
            }
            workspaces.push(abs.clone());
            save_workspaces(&ws_file, &workspaces)?;
            println!("已添加工作区: {}", abs.display());
        }
        WorkspaceSub::List => {
            let workspaces = load_workspaces(&ws_file);
            if workspaces.is_empty() {
                println!("无已授权工作区。使用 workspace add <path> 或 workspace add --cwd 添加。");
                return Ok(());
            }
            for ws in &workspaces {
                println!("{}", ws.display());
            }
        }
        WorkspaceSub::Remove { path } => {
            let abs = normalize_path(&path)?;
            let mut workspaces = load_workspaces(&ws_file);
            let before = workspaces.len();
            workspaces.retain(|p| p != &abs);
            if workspaces.len() == before {
                println!("工作区不存在: {}", abs.display());
                return Ok(());
            }
            save_workspaces(&ws_file, &workspaces)?;
            println!("已移除工作区: {}", abs.display());
        }
    }
    Ok(())
}

fn load_workspaces(path: &Path) -> Vec<PathBuf> {
    if !path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    #[derive(serde::Deserialize)]
    struct WsFile {
        #[serde(default)]
        workspaces: Vec<PathBuf>,
    }
    match serde_json::from_str::<WsFile>(&content) {
        Ok(f) => f.workspaces,
        Err(_) => {
            eprintln!("⚠ ext_workspaces.json 格式损坏，返回空列表");
            Vec::new()
        }
    }
}

fn save_workspaces(path: &Path, workspaces: &[PathBuf]) -> Result<(), AppError> {
    #[derive(serde::Serialize)]
    struct WsFile<'a> {
        workspaces: &'a [PathBuf],
    }
    let json = serde_json::to_string_pretty(&WsFile { workspaces })
        .map_err(|e| AppError::Config(e.to_string()))?;
    write_file_atomic(path, json.as_bytes())
}

struct PluginContext {
    plugin_manager: PluginManager,
    #[allow(dead_code)]
    config: AppConfig,
}

struct NoopToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for NoopToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        Err(AppError::Config(format!(
            "CLI 模式下不支持工具执行: {}",
            tool.name
        )))
    }
}

fn build_plugin_context(cfg: &AppConfig) -> Result<PluginContext, AppError> {
    let event_bus: std::sync::Arc<dyn EventBus> = std::sync::Arc::new(DefaultEventBus::new());
    let executor: std::sync::Arc<dyn ToolExecutor> = std::sync::Arc::new(NoopToolExecutor);
    let audit: std::sync::Arc<dyn crate::infra::AuditRecorder> =
        match AuditStore::open_if_enabled(cfg)? {
            Some(store) => std::sync::Arc::new(FileAuditRecorder::new(std::sync::Arc::new(store))),
            None => std::sync::Arc::new(TracingAuditRecorder),
        };
    let tool_registry: std::sync::Arc<dyn ToolRegistry> =
        std::sync::Arc::new(DefaultToolRegistry::new(executor, audit.clone()));
    let mut pm = PluginManager::new(event_bus);
    pm.set_tool_registry(tool_registry);
    pm.set_audit_recorder(audit);

    let resolved_qjs = resolve_quickjs_path(cfg);
    let wasm_cfg = WasmEngineConfig {
        quickjs_path: resolved_qjs.and_then(|p| p.to_str().map(String::from)),
        ..Default::default()
    };
    if let Ok(engine) = WasmEngine::global(Some(wasm_cfg)) {
        pm.set_wasm_engine(engine);
    }

    type ConfirmFn = dyn Fn(&crate::PluginManifest) -> Result<bool, AppError> + Send + Sync;
    let confirm_fn: std::sync::Arc<ConfirmFn> = std::sync::Arc::new(cli_confirm_permissions);
    pm.set_confirm_permissions(confirm_fn);

    Ok(PluginContext {
        plugin_manager: pm,
        config: cfg.clone(),
    })
}

fn cli_confirm_permissions(manifest: &crate::PluginManifest) -> Result<bool, AppError> {
    use std::io::{self, BufRead, Write};
    println!(
        "插件 {} (v{}) 请求以下权限: {:?}",
        manifest.name, manifest.version, manifest.required_permissions
    );
    print!("是否授权？[y/N] ");
    io::stdout().flush().map_err(AppError::Io)?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(AppError::Io)?;
    let answer = line.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}

fn format_plugin_info(info: &crate::PluginInfo) {
    println!("  ID:        {}", info.id);
    println!("  名称:      {}", info.manifest.name);
    println!("  版本:      {}", info.manifest.version);
    println!("  描述:      {}", info.manifest.description);
    println!("  作者:      {}", info.manifest.author);
    println!("  状态:      {:?}", info.status);
    println!("  权限:      {:?}", info.manifest.required_permissions);
    println!("  API 版本:  {}", info.manifest.required_api_version);
    println!("  注册工具:  {:?}", info.registered_tools);
    println!("  加载时间:  {}", info.loaded_at);
}

// ─── Plugin Registry (registry.json) ──────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PluginRegistryEntry {
    id: String,
    path: String,
    enabled: bool,
    loaded_at: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct PluginRegistryFile {
    #[serde(default)]
    plugins: Vec<PluginRegistryEntry>,
}

fn registry_path(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(resolve_plugins_dir(cfg)?.join("registry.json"))
}

fn load_plugin_registry(path: &Path) -> PluginRegistryFile {
    if !path.exists() {
        return PluginRegistryFile::default();
    }
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| {
            eprintln!("⚠ registry.json 格式损坏，返回空注册表");
            PluginRegistryFile::default()
        }),
        Err(_) => PluginRegistryFile::default(),
    }
}

fn save_plugin_registry(path: &Path, reg: &PluginRegistryFile) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(reg).map_err(|e| AppError::Config(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    write_file_atomic(path, json.as_bytes())
}

pub(crate) fn run_plugin(sub: PluginSub, cfg: &AppConfig) -> Result<(), AppError> {
    let ctx = build_plugin_context(cfg)?;
    let pm = &ctx.plugin_manager;
    let reg_path = registry_path(cfg)?;

    match sub {
        PluginSub::List => {
            let ids = pm.list_loaded();
            let registry = load_plugin_registry(&reg_path);

            if ids.is_empty() && registry.plugins.is_empty() {
                println!("当前无已加载或已注册插件。");
                if !ctx.config.plugin.auto_load.is_empty() {
                    println!(
                        "  提示: auto_load 中的插件将在对话模式启动时自动加载: {:?}",
                        ctx.config.plugin.auto_load
                    );
                }
            } else {
                println!(
                    "{:<20} {:<15} {:<10} {:<10}",
                    "ID", "路径/名称", "启用", "状态"
                );
                println!("{}", "-".repeat(60));
                for id in &ids {
                    if let Some(info) = pm.get_plugin(id) {
                        println!(
                            "{:<20} {:<15} {:<10} {:?}",
                            info.id, info.manifest.name, "loaded", info.status
                        );
                    }
                }
                for entry in &registry.plugins {
                    if !ids.contains(&entry.id) {
                        let status = "registered";
                        println!(
                            "{:<20} {:<15} {:<10} {}",
                            entry.id,
                            entry.path,
                            if entry.enabled { "是" } else { "否" },
                            status
                        );
                    }
                }
            }
        }
        PluginSub::Load { path } => {
            let p = std::path::Path::new(&path);
            if !p.exists() {
                println!("插件路径不存在: {}", path);
                return Ok(());
            }
            match pm.load_plugin(p) {
                Ok(()) => {
                    println!("插件加载成功: {}", path);
                    let ids = pm.list_loaded();
                    if let Some(id) = ids.last() {
                        if let Some(info) = pm.get_plugin(id) {
                            format_plugin_info(&info);
                        }
                        let mut registry = load_plugin_registry(&reg_path);
                        registry.plugins.retain(|e| e.id != *id);
                        registry.plugins.push(PluginRegistryEntry {
                            id: id.clone(),
                            path: path.clone(),
                            enabled: true,
                            loaded_at: chrono::Utc::now().to_rfc3339(),
                        });
                        save_plugin_registry(&reg_path, &registry)?;
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    println!("插件加载失败: {}", msg);
                    if msg.contains("WasmEdge") || msg.contains("wasm_engine") {
                        println!("  提示: 请先运行 pi doctor 检查运行环境");
                    }
                }
            }
        }
        PluginSub::Unload { id } => match pm.unload_plugin(&id) {
            Ok(()) => {
                println!("已卸载插件: {}", id);
                let mut registry = load_plugin_registry(&reg_path);
                registry.plugins.retain(|e| e.id != id);
                save_plugin_registry(&reg_path, &registry)?;
            }
            Err(e) => println!("卸载失败: {}", e),
        },
        PluginSub::Enable { id } => match pm.enable_plugin(&id) {
            Ok(()) => {
                println!("已启用插件: {}", id);
                let mut registry = load_plugin_registry(&reg_path);
                if let Some(entry) = registry.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = true;
                    save_plugin_registry(&reg_path, &registry)?;
                }
            }
            Err(e) => println!("启用失败: {}", e),
        },
        PluginSub::Disable { id } => match pm.disable_plugin(&id) {
            Ok(()) => {
                println!("已禁用插件: {}", id);
                let mut registry = load_plugin_registry(&reg_path);
                if let Some(entry) = registry.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = false;
                    save_plugin_registry(&reg_path, &registry)?;
                }
            }
            Err(e) => println!("禁用失败: {}", e),
        },
        PluginSub::Info { id } => match pm.get_plugin(&id) {
            Some(info) => format_plugin_info(&info),
            None => println!("插件未找到: {}", id),
        },
    }
    Ok(())
}

#[allow(dead_code)] // 保留供单元测试（旧 tracing 日志解析格式）
#[derive(Debug, Clone, serde::Serialize)]
struct AuditDisplayEntry {
    index: usize,
    timestamp: String,
    audit_type: String,
    detail: String,
    success: String,
}

#[allow(dead_code)] // 保留供单元测试
fn parse_audit_line(line: &str, index: usize) -> Option<AuditDisplayEntry> {
    let audit_type = if line.contains("audit primitive") {
        wire::WIRE_AUDIT_PRIMITIVE
    } else if line.contains("audit tool_call") {
        wire::WIRE_TOOL_CALL
    } else if line.contains("audit hostcall") {
        wire::WIRE_AUDIT_HOSTCALL
    } else {
        return None;
    };

    let timestamp = line
        .find(char::is_numeric)
        .and_then(|start| line.get(start..start + 30.min(line.len() - start)))
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("unknown")
        .to_string();

    let success = if line.contains("success=true") || line.contains("success: true") {
        "OK"
    } else if line.contains("success=false") || line.contains("success: false") {
        "FAIL"
    } else {
        "?"
    };

    let detail = line
        .find("operation=")
        .or_else(|| line.find("tool_name="))
        .or_else(|| line.find("module="))
        .map(|start| {
            let end = line.len().min(start + 80);
            line[start..end].to_string()
        })
        .unwrap_or_else(|| {
            let trimmed = line.trim();
            let end = trimmed.len().min(80);
            trimmed[..end].to_string()
        });

    Some(AuditDisplayEntry {
        index,
        timestamp,
        audit_type: audit_type.to_string(),
        detail,
        success: success.to_string(),
    })
}

#[allow(dead_code)] // 保留供测试或兼容旧 tracing 日志解析
fn find_latest_log_file(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .max_by_key(|p| {
            p.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

#[allow(dead_code)] // 保留供单元测试
fn read_audit_entries(
    log_path: &std::path::Path,
    limit: Option<u32>,
) -> Result<Vec<AuditDisplayEntry>, AppError> {
    use std::io::BufRead;
    let file = std::fs::File::open(log_path).map_err(AppError::Io)?;
    let reader = std::io::BufReader::new(file);
    let mut entries = Vec::new();
    let mut audit_index = 0usize;
    for line in reader.lines() {
        let line = line.map_err(AppError::Io)?;
        if let Some(entry) = parse_audit_line(&line, audit_index) {
            audit_index += 1;
            entries.push(entry);
        }
    }
    entries.reverse();
    let max = limit.unwrap_or(50) as usize;
    entries.truncate(max);
    Ok(entries)
}

pub(crate) fn run_audit(sub: AuditSub, cfg: &AppConfig) -> Result<(), AppError> {
    if !cfg.security.enable_audit_log {
        println!("审计日志未开启。请在配置中设置 security.enable_audit_log = true");
        return Ok(());
    }
    let store = match AuditStore::new(cfg) {
        Ok(s) => s,
        Err(e) => {
            println!("无法打开审计存储: {}", e);
            return Ok(());
        }
    };
    let audit_dir = resolve_audit_dir(cfg)?;
    if !audit_dir.exists() {
        println!("审计目录不存在: {}，尚无审计记录", audit_dir.display());
        return Ok(());
    }

    match sub {
        AuditSub::List { limit } => {
            let _ = store.cleanup();
            let filter = AuditFilter {
                limit: limit.or(Some(50)),
                ..Default::default()
            };
            let entries = store.query(&filter)?;
            if entries.is_empty() {
                println!("未找到审计记录");
                return Ok(());
            }
            println!(
                "{:<6} {:<28} {:<14} {:<6} 详情",
                "序号", "时间", "类型", "状态"
            );
            println!("{}", "-".repeat(90));
            for e in &entries {
                let status = if e.success() { "OK" } else { "FAIL" };
                println!(
                    "{:<6} {:<28} {:<14} {:<6} {}",
                    e.id,
                    e.timestamp,
                    e.kind_label(),
                    status,
                    e.detail_short()
                );
            }
            println!("共 {} 条", entries.len());
        }
        AuditSub::Show { id } => {
            let idx: u64 = id.parse().unwrap_or(0);
            let filter = AuditFilter {
                limit: None,
                ..Default::default()
            };
            let entries = store.query(&filter)?;
            match entries.iter().find(|e| e.id == idx) {
                Some(e) => {
                    let status = if e.success() { "OK" } else { "FAIL" };
                    println!("序号:   {}", e.id);
                    println!("时间:   {}", e.timestamp);
                    println!("类型:   {}", e.kind_label());
                    println!("状态:   {}", status);
                    println!("详情:   {}", e.detail_short());
                }
                None => {
                    println!("未找到审计记录: {}", id);
                }
            }
        }
        AuditSub::Export { path } => {
            let filter = AuditFilter {
                limit: None,
                ..Default::default()
            };
            let entries = store.query(&filter)?;
            if entries.is_empty() {
                println!("无审计记录可导出");
                return Ok(());
            }
            store.export_to(&path)?;
            println!("已导出 {} 条审计记录到 {}", entries.len(), path.display());
        }
    }
    Ok(())
}

pub(crate) fn run_chat(resume: bool, cfg: &AppConfig) -> Result<(), AppError> {
    let ctx = super::chat::ChatContext::from_config(cfg.clone())?;

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Config(format!("创建 tokio 运行时失败: {}", e)))?;

    let cancelled = ctx.cancelled.clone();
    ctrlc::set_handler(move || {
        cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    })
    .ok();

    rt.block_on(super::chat::chat_loop(&ctx, resume))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire;

    fn test_config(dir: &std::path::Path) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(dir.to_str().unwrap().to_string());
        cfg
    }

    #[test]
    fn cli_parse_init() {
        let cli = Cli::try_parse_from(["pi", "init"]).unwrap();
        let cmd = cli.command.expect("subcommand");
        assert!(matches!(cmd, Commands::Init));
    }

    #[test]
    fn cli_parse_init_rejects_config_flag() {
        let r = Cli::try_parse_from(["pi", "init", "--config", "/tmp/pi.config.toml"]);
        assert!(r.is_err(), "--config should be rejected after removal");
    }

    #[test]
    fn cli_parse_doctor() {
        let cli = Cli::try_parse_from(["pi", "doctor"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Doctor)));
    }

    #[test]
    fn cli_parse_config_get() {
        let cli = Cli::try_parse_from(["pi", "config", "get"]).unwrap();
        let cmd = cli.command.unwrap();
        if let Commands::Config { sub } = cmd {
            assert!(matches!(sub, ConfigSub::Get { key: None }));
        }
    }

    #[test]
    fn cli_parse_session_list() {
        let cli = Cli::try_parse_from(["pi", "session", "list"]).unwrap();
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
        let cli = Cli::try_parse_from(["pi", "plugin", "list"]).unwrap();
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
        let cli = Cli::try_parse_from(["pi", "audit", "list"]).unwrap();
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
        let cli = Cli::try_parse_from(["pi"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn run_init_returns_ok() {
        let r = run_init();
        assert!(r.is_ok());
    }

    #[test]
    fn run_doctor_returns_ok() {
        let r = run_doctor();
        assert!(r.is_ok());
    }

    #[test]
    fn run_plugin_list_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_plugin(PluginSub::List, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_audit_list_returns_ok() {
        let cfg = AppConfig::default();
        let r = run_audit(AuditSub::List { limit: None }, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_get_with_key_returns_ok() {
        let cfg = AppConfig::default();
        let r = run_config(
            ConfigSub::Get {
                key: Some("log.level".to_string()),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_get_without_key_returns_ok() {
        let cfg = AppConfig::default();
        let r = run_config(ConfigSub::Get { key: None }, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_set_returns_ok() {
        let cfg = AppConfig::default();
        let r = run_config(
            ConfigSub::Set {
                key: "log.level".to_string(),
                value: "debug".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_config_edit_returns_ok() {
        run_init().unwrap();

        let old_editor = std::env::var("EDITOR").ok();
        if cfg!(unix) {
            std::env::set_var("EDITOR", "true");
        } else {
            std::env::set_var("EDITOR", "cmd /c exit 0");
        }

        let cfg = AppConfig::default();
        let r = run_config(ConfigSub::Edit, &cfg);

        match old_editor {
            Some(v) => std::env::set_var("EDITOR", v),
            None => std::env::remove_var("EDITOR"),
        }
        assert!(r.is_ok());
    }

    #[test]
    fn run_doctor_is_always_ok() {
        let r = run_doctor();
        assert!(r.is_ok());
    }

    // --- session tests (direct AppConfig, no env vars) ---

    #[test]
    fn run_session_list_empty_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_session(SessionSub::List, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_new_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_session(SessionSub::New, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_list_after_new_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let _ = run_session(SessionSub::New, &cfg);
        let r = run_session(SessionSub::List, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_switch_nonexistent_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_session(
            SessionSub::Switch {
                key: "nonexistent".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_switch_existing_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let _ = run_session(SessionSub::New, &cfg);
        let r = run_session(
            SessionSub::Switch {
                key: crate::DEFAULT_SESSION_KEY.to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_delete_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let _ = run_session(SessionSub::New, &cfg);
        let r = run_session(
            SessionSub::Delete {
                key: crate::DEFAULT_SESSION_KEY.to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok(), "run_session(Delete) failed: {:?}", r);
    }

    #[test]
    fn run_session_archive_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let _ = run_session(SessionSub::New, &cfg);
        let r = run_session(
            SessionSub::Archive {
                key: crate::DEFAULT_SESSION_KEY.to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_search_empty_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_session(SessionSub::Search { query: None }, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_session_search_with_query_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_session(
            SessionSub::Search {
                query: Some("q".to_string()),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    // --- workspace tests ---

    #[test]
    fn run_workspace_add_list_remove() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path.clone()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::List, &cfg);
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::Remove { path: target_path }, &cfg);
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::List, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_workspace_add_nonexistent_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some("/nonexistent/path/should/fail".to_string()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_err());
    }

    #[test]
    fn run_workspace_add_duplicate_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path.clone()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_workspace_add_cwd_adds_current_dir() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(target.path()).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(target.path()).unwrap();
        let r = run_workspace(
            WorkspaceSub::Add {
                path: None,
                cwd: true,
            },
            &cfg,
        );
        std::env::set_current_dir(&prev).unwrap();
        assert!(r.is_ok());

        let agent_dir = resolve_agent_dir(&cfg).unwrap();
        let ws_file = agent_dir.join("ext_workspaces.json");
        let list = super::load_workspaces(&ws_file);
        assert!(list.iter().any(|p| p == &canon));
    }

    #[test]
    fn run_workspace_remove_nonexistent_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();

        let r = run_workspace(
            WorkspaceSub::Remove {
                path: "/some/path".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    // --- plugin registry tests ---

    #[test]
    fn plugin_registry_load_save_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");

        let reg = load_plugin_registry(&path);
        assert!(reg.plugins.is_empty());

        let mut reg = PluginRegistryFile::default();
        reg.plugins.push(PluginRegistryEntry {
            id: "test-plugin".to_string(),
            path: "/some/path".to_string(),
            enabled: true,
            loaded_at: "2026-01-01T00:00:00Z".to_string(),
        });
        save_plugin_registry(&path, &reg).unwrap();

        let loaded = load_plugin_registry(&path);
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.plugins[0].id, "test-plugin");
        assert!(loaded.plugins[0].enabled);
    }

    #[test]
    fn plugin_registry_corrupt_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        std::fs::write(&path, "not valid json {{{").unwrap();

        let reg = load_plugin_registry(&path);
        assert!(reg.plugins.is_empty());
    }

    #[test]
    fn run_audit_show_and_export_returns_ok() {
        let cfg = AppConfig::default();
        let r = run_audit(
            AuditSub::Show {
                id: "id1".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
        let dir = tempfile::tempdir().unwrap();
        let r2 = run_audit(
            AuditSub::Export {
                path: dir.path().join("audit.json"),
            },
            &cfg,
        );
        assert!(r2.is_ok());
    }

    // --- doctor tests ---

    #[test]
    fn run_doctor_after_init_returns_ok() {
        run_init().unwrap();
        let r = run_doctor();
        assert!(r.is_ok());
    }

    // --- config get/set/edit tests ---

    #[test]
    fn resolve_toml_key_finds_nested() {
        let cfg = AppConfig::default();
        let val = toml::Value::try_from(&cfg).unwrap();
        let found = resolve_toml_key(&val, "log.level");
        assert!(found.is_some());
        assert_eq!(found.unwrap().as_str().unwrap(), "info");
    }

    #[test]
    fn resolve_toml_key_returns_none_for_missing() {
        let cfg = AppConfig::default();
        let val = toml::Value::try_from(&cfg).unwrap();
        assert!(resolve_toml_key(&val, "nonexistent.key").is_none());
    }

    #[test]
    fn set_toml_key_changes_string_value() {
        let cfg = AppConfig::default();
        let mut val = toml::Value::try_from(&cfg).unwrap();
        let r = set_toml_key(&mut val, "log.level", "debug");
        assert!(r.is_ok());
        let found = resolve_toml_key(&val, "log.level").unwrap();
        assert_eq!(found.as_str().unwrap(), "debug");
    }

    #[test]
    fn set_toml_key_changes_integer_value() {
        let cfg = AppConfig::default();
        let mut val = toml::Value::try_from(&cfg).unwrap();
        let r = set_toml_key(&mut val, "security.audit_log_retention_days", "30");
        assert!(r.is_ok());
        let found = resolve_toml_key(&val, "security.audit_log_retention_days").unwrap();
        assert_eq!(found.as_integer().unwrap(), 30);
    }

    #[test]
    fn set_toml_key_changes_bool_value() {
        let cfg = AppConfig::default();
        let mut val = toml::Value::try_from(&cfg).unwrap();
        let r = set_toml_key(&mut val, "log.file_enabled", "true");
        assert!(r.is_ok());
        let found = resolve_toml_key(&val, "log.file_enabled").unwrap();
        assert!(found.as_bool().unwrap());
    }

    #[test]
    fn set_toml_key_rejects_nonexistent_path() {
        let cfg = AppConfig::default();
        let mut val = toml::Value::try_from(&cfg).unwrap();
        let r = set_toml_key(&mut val, "nonexistent.key", "val");
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("不存在"));
    }

    #[test]
    fn set_toml_key_rejects_bad_integer() {
        let cfg = AppConfig::default();
        let mut val = toml::Value::try_from(&cfg).unwrap();
        let r = set_toml_key(
            &mut val,
            "security.audit_log_retention_days",
            "not_a_number",
        );
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("整数"));
    }

    #[test]
    fn config_set_with_real_file() {
        run_init().unwrap();
        let config_path = normalize_path(DEFAULT_CONFIG_PATH).unwrap();
        assert!(config_path.exists(), "config should exist after init");
    }

    #[test]
    fn config_file_path_resolves_default() {
        let r = config_file_path();
        assert!(r.is_ok());
    }

    // --- parse_audit_line tests ---

    #[test]
    fn parse_audit_line_matches_primitive() {
        let line = r#"2025-03-10T12:00:00Z  INFO audit primitive operation=Read path_or_cmd=/tmp/foo plugin_id=p1 user_approved=true success=true"#;
        let entry = parse_audit_line(line, 0);
        assert!(entry.is_some());
        let e = entry.unwrap();
        assert_eq!(e.audit_type, wire::WIRE_AUDIT_PRIMITIVE);
        assert_eq!(e.success, "OK");
    }

    #[test]
    fn parse_audit_line_matches_tool_call() {
        let line = r#"2025-03-10T12:00:00Z  INFO audit tool_call tool_name=run success=false"#;
        let entry = parse_audit_line(line, 1);
        assert!(entry.is_some());
        let e = entry.unwrap();
        assert_eq!(e.audit_type, wire::WIRE_TOOL_CALL);
        assert_eq!(e.success, "FAIL");
    }

    #[test]
    fn parse_audit_line_matches_hostcall() {
        let line =
            r#"2025-03-10T12:00:00Z  INFO audit hostcall module=fs method=readFile success=true"#;
        let entry = parse_audit_line(line, 2);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().audit_type, wire::WIRE_AUDIT_HOSTCALL);
    }

    #[test]
    fn parse_audit_line_returns_none_for_non_audit() {
        let line = "2025-03-10T12:00:00Z  INFO some other log line";
        assert!(parse_audit_line(line, 0).is_none());
    }

    #[test]
    fn read_audit_entries_from_file_with_audit_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        std::fs::write(
            &log,
            "line1\n2025-01-01 INFO audit primitive operation=Read success=true\nline3\n2025-01-02 INFO audit tool_call tool_name=x success=false\n",
        )
        .unwrap();
        let entries = read_audit_entries(&log, Some(10)).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].audit_type, wire::WIRE_TOOL_CALL);
        assert_eq!(entries[1].audit_type, wire::WIRE_AUDIT_PRIMITIVE);
    }

    #[test]
    fn read_audit_entries_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("empty.log");
        std::fs::write(&log, "no audit here\njust logs\n").unwrap();
        let entries = read_audit_entries(&log, None).unwrap();
        assert!(entries.is_empty());
    }

    // --- plugin tests ---

    #[test]
    fn run_plugin_list_returns_ok_with_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_plugin(PluginSub::List, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn run_plugin_load_nonexistent_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_plugin(
            PluginSub::Load {
                path: "/nonexistent/path/to/plugin".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_plugin_info_not_found_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_plugin(
            PluginSub::Info {
                id: "nonexistent-plugin".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_plugin_unload_not_found_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_plugin(
            PluginSub::Unload {
                id: "nonexistent-plugin".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_plugin_enable_not_found_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_plugin(
            PluginSub::Enable {
                id: "nonexistent-plugin".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn run_plugin_disable_not_found_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        let r = run_plugin(
            PluginSub::Disable {
                id: "nonexistent-plugin".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    }

    // --- audit with enable_audit_log = false ---

    #[test]
    fn run_audit_list_file_disabled_returns_ok() {
        let mut cfg = AppConfig::default();
        cfg.security.enable_audit_log = false;
        let r = run_audit(AuditSub::List { limit: None }, &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn audit_export_with_entries() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        std::fs::write(
            &log,
            "2025-01-01 INFO audit primitive operation=Read success=true\n",
        )
        .unwrap();
        let export_path = dir.path().join("out.json");
        let entries = read_audit_entries(&log, None).unwrap();
        assert!(!entries.is_empty());
        let json = serde_json::to_string_pretty(&entries).unwrap();
        std::fs::write(&export_path, &json).unwrap();
        let content = std::fs::read_to_string(&export_path).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.len(), 1);
    }
}
