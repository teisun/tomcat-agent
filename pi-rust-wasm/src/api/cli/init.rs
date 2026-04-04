//! `pi init` 与 `pi doctor` 子命令实现。

use std::io::Write;
use std::path::Path;

use crate::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, load_config, normalize_path,
    resolve_quickjs_path, validate_config, AppConfig, AppError, WasmEngine, WasmEngineConfig,
    DEFAULT_LLM_MODEL,
};

use super::DEFAULT_CONFIG_PATH;

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
