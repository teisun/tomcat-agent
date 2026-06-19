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
        crate::load_config(Some(&config_file))?
    } else {
        let llm = crate::LlmConfig {
            default_model: DEFAULT_LLM_MODEL.to_string(),
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
    println!("  ✓ 默认模型: {}", cfg.llm.default_model);
    println!("  ✓ 默认模型协议线: {}", model_choice.entry.api);
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

    match crate::api::cli::models_toml::ensure_default_models_toml(&cfg)? {
        crate::api::cli::models_toml::ModelsTomlStatus::Created { added_model_ids } => {
            println!(
                "  ✓ 已生成模型清单 models.toml（含 {}）",
                added_model_ids.join(", ")
            )
        }
        crate::api::cli::models_toml::ModelsTomlStatus::UpdatedExisting {
            added_model_ids,
            updated_model_name_ids,
        } => match (
            added_model_ids.is_empty(),
            updated_model_name_ids.is_empty(),
        ) {
            (false, false) => println!(
                "  ✓ 已向现有 models.toml 补齐受管默认模型：{}；并补写 model_name：{}",
                added_model_ids.join(", "),
                updated_model_name_ids.join(", ")
            ),
            (false, true) => println!(
                "  ✓ 已向现有 models.toml 补齐受管默认模型：{}",
                added_model_ids.join(", ")
            ),
            (true, false) => println!(
                "  ✓ 已为现有 models.toml 补写受管默认模型的 model_name：{}",
                updated_model_name_ids.join(", ")
            ),
            (true, true) => println!("  ✓ models.toml 已就绪（受管默认模型已齐全）"),
        },
        crate::api::cli::models_toml::ModelsTomlStatus::AlreadyPresent => {
            println!("  ✓ models.toml 已就绪（受管默认模型与 model_name 已齐全）")
        }
    }

    match crate::api::cli::builtin_plugins::ensure_builtin_plugins(&cfg)? {
        crate::api::cli::builtin_plugins::BuiltinPluginsStatus::Created => {
            println!("  ✓ 已安装官方插件 web-search-backends")
        }
        crate::api::cli::builtin_plugins::BuiltinPluginsStatus::UpdatedExistingPlugin => {
            println!("  ✓ 已更新官方插件 web-search-backends 的缺失文件/关键 manifest 字段")
        }
        crate::api::cli::builtin_plugins::BuiltinPluginsStatus::AlreadyPresent => {
            println!("  ✓ 官方插件 web-search-backends 已就绪")
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
        &model_choice.env_name,
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
    for line in doctor_proxy_lines(cfg) {
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
        let key_env = crate::core::llm::ModelCatalog::load(cfg)
            .ok()
            .and_then(|catalog| catalog.lookup(&cfg.llm.default_model).cloned())
            .map(|entry| {
                entry
                    .api_key_env
                    .unwrap_or_else(|| crate::core::llm::env_name_for_provider(&entry.provider))
            })
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

struct ProxyEnvDiagnostic {
    key: &'static str,
    value: String,
    had_whitespace: bool,
}

fn proxy_env_diagnostics() -> Vec<ProxyEnvDiagnostic> {
    ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"]
        .into_iter()
        .filter_map(|key| {
            let raw = std::env::var(key).ok()?;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(ProxyEnvDiagnostic {
                key,
                value: trimmed.to_string(),
                had_whitespace: raw != trimmed,
            })
        })
        .collect()
}

pub(crate) fn doctor_proxy_lines(cfg: &AppConfig) -> Vec<String> {
    let env_proxies = proxy_env_diagnostics();
    let configured_proxy = cfg.llm.proxy.as_deref().and_then(|proxy| {
        let trimmed = proxy.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some((proxy, trimmed))
        }
    });
    let mut lines = Vec::new();

    match configured_proxy {
        Some((raw, trimmed)) => {
            lines.push("✓ llm.proxy 已配置，将优先于环境代理".to_string());
            if raw != trimmed {
                lines.push("⚠ llm.proxy 含首尾空格；运行时会 trim，建议清理配置".to_string());
            }
        }
        None if !env_proxies.is_empty() => {
            let keys = env_proxies
                .iter()
                .map(|item| item.key)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!(
                "✓ 检测到环境代理（{keys}）；未配置 llm.proxy 时，出网请求将由环境变量生效"
            ));
        }
        None => {
            lines.push("✓ 未检测到 llm.proxy 或环境代理；出网请求将直连".to_string());
        }
    }

    for item in env_proxies {
        if item.had_whitespace {
            lines.push(format!("⚠ {} 含首尾空格；建议清理配置", item.key));
        }
        if item.key == "ALL_PROXY" && item.value.to_ascii_lowercase().starts_with("socks5://") {
            lines.push(
                "⚠ ALL_PROXY 使用 socks5://，当前构建未启用 reqwest socks feature；web_search 建议改用 HTTPS_PROXY=http://..."
                    .to_string(),
            );
        }
    }

    lines
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
