//! `tomcat init` 与 `tomcat doctor` 子命令实现。

use std::io::Write;
use std::path::Path;

use crate::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, load_config, load_store,
    normalize_path, resolve_sessions_dir, validate_config, AppConfig, AppError, PluginEngine,
    DEFAULT_LLM_MODEL,
};

use super::DEFAULT_CONFIG_PATH;

pub(crate) fn run_init() -> Result<(), AppError> {
    let config_file = normalize_path(DEFAULT_CONFIG_PATH)?;

    // --- [1/3] 环境初始化（标题先于配置写入，便于失败时仍可见步骤）---
    println!("\n[1/3] 环境初始化");

    let config_existed = config_file.exists();
    if config_existed {
        println!(
            "  已存在配置文件，将以现有内容为基线更新：{}",
            config_file.display()
        );
    }

    let mut cfg = if config_existed {
        crate::load_config(Some(&config_file)).unwrap_or_default()
    } else {
        let llm = crate::LlmConfig {
            provider: "openai-responses".to_string(),
            default_model: DEFAULT_LLM_MODEL.to_string(),
            api_base: None,
            ..Default::default()
        };
        AppConfig {
            llm,
            ..Default::default()
        }
    };
    let model_catalog = crate::core::llm::ModelCatalog::load(&cfg)?;
    let model_choice =
        crate::api::cli::init_model_wizard::run_model_wizard(&mut cfg, &model_catalog)?;

    if let Some(parent) = config_file.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let toml_str = toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(&config_file, toml_str).map_err(AppError::Io)?;

    if config_existed {
        println!("  ✓ 配置文件已更新: {}", config_file.display());
    } else {
        println!("  ✓ 配置文件已写入: {}", config_file.display());
    }
    println!("  ✓ 默认 LLM Provider: {}", cfg.llm.provider);
    println!("  ✓ 默认模型: {}", cfg.llm.default_model);
    println!("  ✓ 模型逻辑厂商: {}", model_choice.entry.provider);
    println!("  ✓ 当前模型凭证变量: {}", model_choice.env_name);

    ensure_work_dir_structure(&cfg)?;
    println!("  ✓ 目录结构就绪");
    let sessions_path = resolve_sessions_dir(&cfg)?.join("sessions.json");
    let store = load_store(&sessions_path)?;
    if store.is_empty() {
        println!("  ✓ sessions.json 已初始化");
    } else {
        println!("  ✓ sessions.json 已保留（{} 个历史会话）", store.len());
    }

    match crate::api::cli::models_toml::ensure_mimo_models_toml(&cfg)? {
        crate::api::cli::models_toml::ModelsTomlStatus::Created => {
            println!("  ✓ 已生成模型清单 models.toml（含 mimo-v2.5-pro）")
        }
        crate::api::cli::models_toml::ModelsTomlStatus::AppendedMimo => {
            println!("  ✓ 已向现有 models.toml 追加 mimo-v2.5-pro")
        }
        crate::api::cli::models_toml::ModelsTomlStatus::AlreadyPresent => {
            println!("  ✓ models.toml 已就绪（已含 mimo-v2.5-pro）")
        }
    }

    ensure_embedded_assets(&cfg)?;
    println!("  ✓ 内嵌资源目录已就绪");

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

    // --- [2/3] 资源检查（与 tomcat doctor 一致，跳过 API Key）---
    println!("\n[2/3] 资源检查");
    run_doctor_checks(&cfg, config_file.as_path(), true)?;

    // --- [3/3] API Key 配置 ---
    println!("\n[3/3] API Key 配置");
    let work_dir = get_work_dir(&cfg)?;
    let env_path = work_dir.join("assets").join(".env");
    match crate::api::cli::init_model_wizard::prompt_and_store_provider_key(
        &env_path,
        &model_choice.env_name,
    )? {
        crate::api::cli::init_model_wizard::KeyConfigStatus::AlreadyConfigured => {
            println!("  ✓ API Key 已配置 ({})", model_choice.env_name);
        }
        crate::api::cli::init_model_wizard::KeyConfigStatus::Written => {
            println!("  ✓ {} 已写入 .env", model_choice.env_name);
        }
        crate::api::cli::init_model_wizard::KeyConfigStatus::Skipped => {
            println!(
                "  ⚠ {} 未设置，后续可运行 `tomcat init` 重新配置，或编辑 {}",
                model_choice.env_name,
                env_path.display()
            );
        }
    }
    let additional_envs = crate::api::cli::init_model_wizard::additional_provider_env_names(
        &model_catalog,
        &model_choice.entry.provider,
    );
    for (env_name, status) in crate::api::cli::init_model_wizard::prompt_additional_provider_keys(
        &env_path,
        &additional_envs,
    )? {
        match status {
            crate::api::cli::init_model_wizard::KeyConfigStatus::AlreadyConfigured => {
                println!("  ✓ API Key 已配置 ({})", env_name);
            }
            crate::api::cli::init_model_wizard::KeyConfigStatus::Written => {
                println!("  ✓ {} 已写入 .env", env_name);
            }
            crate::api::cli::init_model_wizard::KeyConfigStatus::Skipped => {
                println!("  ⚠ {} 未设置，已跳过", env_name);
            }
        }
    }

    println!("\n初始化完成！运行 `tomcat code` 开始对话。");

    Ok(())
}

/// 将 `tomcat` 可执行文件所在目录追加到 shell 启动脚本中的 PATH；已存在同序 export 则跳过。
fn auto_add_to_path(bin_dir: &Path) -> bool {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let Some(home) = crate::infra::platform::home_dir() else {
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
    writeln!(f, "\n# Added by tomcat init\n{}", export_line).is_ok()
}

/// 与 `tomcat doctor` 相同的逐项检查。`skip_api_key` 为 true 时（用于 `tomcat init` 第二步）不输出 .env 权限与 OPENAI_API_KEY 相关行。
pub(crate) fn run_doctor_checks(
    cfg: &AppConfig,
    config_path: &Path,
    skip_api_key: bool,
) -> Result<(), AppError> {
    if let Err(e) = validate_config(cfg) {
        println!("✗ 配置不合法: {}", e);
        println!(
            "  → 运行 tomcat init 重新生成或手动修复 {}",
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
        println!("  → 运行 tomcat init 或检查磁盘空间");
    } else {
        println!("✓ 内嵌资源已就绪");
    }

    // --- rquickjs 运行时 ---
    for line in doctor_plugin_runtime_lines(PluginEngine::global(None).map(|_| ())) {
        println!("{line}");
    }

    if !skip_api_key {
        // --- .env 检查 ---
        let work_dir = get_work_dir(cfg)?;
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
            println!("  → 运行 tomcat init 配置 API Key");
        }

        // --- 当前默认模型所需 API Key ---
        let key_env = cfg
            .llm
            .api_key_env
            .clone()
            .filter(|env| !env.trim().is_empty())
            .unwrap_or_else(|| "OPENAI_API_KEY".to_string());
        match std::env::var(&key_env) {
            Ok(k) if !k.is_empty() => println!("✓ {} 已设置", key_env),
            _ => {
                println!("⚠ {} 未设置", key_env);
                println!("  → 运行 tomcat init 或编辑 {}", env_path.display());
            }
        }
    }

    Ok(())
}

pub(crate) fn doctor_plugin_runtime_lines(probe: Result<(), AppError>) -> Vec<String> {
    match probe {
        Ok(()) => vec!["✓ rquickjs 运行时：可用".to_string()],
        Err(e) => vec![
            format!("✗ rquickjs 运行时：初始化失败 ({})", e),
            "  → 重新运行 tomcat init；若问题持续，请检查嵌入资源与本地构建产物".to_string(),
        ],
    }
}

pub(crate) fn run_doctor() -> Result<(), AppError> {
    let path = match normalize_path(DEFAULT_CONFIG_PATH) {
        Ok(p) if p.exists() => p,
        _ => {
            println!("✗ 未找到配置文件");
            println!("  → 运行 tomcat init 生成配置");
            return Ok(());
        }
    };
    let cfg = match load_config(Some(path.as_path())) {
        Ok(cfg) => cfg,
        Err(e) => {
            println!("✗ 配置加载失败: {}", e);
            println!("  → 运行 tomcat init 重新生成或手动修复 {}", path.display());
            return Ok(());
        }
    };
    run_doctor_checks(&cfg, path.as_path(), false)?;
    Ok(())
}
