//! `tomcat init` 与 `tomcat doctor` 子命令实现。

use crate::infra::i18n::tr;
use crate::infra::platform::{
    add_user_bin_dir_to_path, canonicalize, install_user_command, manual_path_step, mode_bits,
    posix_shell, user_bin_dir, ManualPathStep, POSIX_SHELL_NEEDS_INSTALL,
};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use crate::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, load_config, load_store,
    normalize_path, resolve_sessions_dir, save_store, validate_config, AppConfig, AppError,
    PluginEngine, SessionStore,
};

use super::DEFAULT_CONFIG_PATH;

pub(crate) fn run_init() -> Result<(), AppError> {
    let config_file = normalize_path(DEFAULT_CONFIG_PATH)?;

    // --- [1/3] 环境初始化（标题先于配置写入，便于失败时仍可见步骤）---
    println!("{}", tr("cli.init.stageSetup", &[]));

    let config_existed = config_file.exists();
    if config_existed {
        println!(
            "{}",
            tr(
                "cli.init.configExisting",
                &[("path", &config_file.display().to_string())]
            )
        );
    }

    let mut cfg = if config_existed {
        load_config(Some(&config_file))?
    } else {
        AppConfig::default()
    };
    let models_toml_status = crate::api::cli::models_toml::ensure_default_models_toml(&cfg)?;
    let model_catalog = crate::core::llm::ModelCatalog::load(&cfg)?;
    let model_choice =
        crate::api::cli::init_model_wizard::run_model_wizard(&mut cfg, &model_catalog)?;

    if let Some(parent) = config_file.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let toml_str = toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(&config_file, toml_str).map_err(AppError::Io)?;

    if config_existed {
        println!(
            "{}",
            tr(
                "cli.init.configUpdated",
                &[("path", &config_file.display().to_string())]
            )
        );
    } else {
        println!(
            "{}",
            tr(
                "cli.init.configWritten",
                &[("path", &config_file.display().to_string())]
            )
        );
    }
    println!(
        "{}",
        tr("cli.init.model", &[("id", &cfg.llm.default_model)])
    );
    println!(
        "{}",
        tr("cli.init.api", &[("api", &model_choice.entry.api)])
    );
    println!(
        "{}",
        tr(
            "cli.init.provider",
            &[("provider", &model_choice.entry.provider)]
        )
    );
    println!(
        "{}",
        tr("cli.init.keyName", &[("name", &model_choice.env_name)])
    );

    ensure_work_dir_structure(&cfg)?;
    println!("{}", tr("cli.init.directories", &[]));
    let mcp_config_path = crate::core::connector::mcp::builtin::materialize_default_mcp_json(&cfg)?;
    println!(
        "{}",
        tr(
            "cli.init.mcp",
            &[("path", &mcp_config_path.display().to_string())]
        )
    );
    let sessions_path = resolve_sessions_dir(&cfg)?.join("sessions.json");
    let store = crate::core::session::store::with_store_write_lock(&sessions_path, || {
        match load_store(&sessions_path) {
            Ok(store) => Ok(store),
            // Explicit init recovery shares the same RMW lock as normal session mutations.
            Err(AppError::Config(_)) => {
                save_store(&sessions_path, &SessionStore::new())?;
                println!("{}", tr("cli.init.sessionsReset", &[]));
                Ok(SessionStore::new())
            }
            Err(error) => Err(error),
        }
    })?;
    if store.is_empty() {
        println!("{}", tr("cli.init.sessionsCreated", &[]));
    } else {
        println!(
            "{}",
            tr(
                "cli.init.sessionsKept",
                &[("count", &store.len().to_string())]
            )
        );
    }

    match models_toml_status {
        crate::api::cli::models_toml::ModelsTomlStatus::Created { added_model_ids } => {
            println!(
                "{}",
                tr(
                    "cli.init.modelsCreated",
                    &[("models", &added_model_ids.join(", "))]
                )
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
                "{}",
                tr(
                    "cli.init.modelsAddedNames",
                    &[
                        ("added", &added_model_ids.join(", ")),
                        ("names", &updated_model_name_ids.join(", "))
                    ]
                )
            ),
            (false, true) => println!(
                "{}",
                tr(
                    "cli.init.modelsAdded",
                    &[("added", &added_model_ids.join(", "))]
                )
            ),
            (true, false) => println!(
                "{}",
                tr(
                    "cli.init.modelsNamed",
                    &[("names", &updated_model_name_ids.join(", "))]
                )
            ),
            (true, true) => println!("{}", tr("cli.init.modelsComplete", &[])),
        },
        crate::api::cli::models_toml::ModelsTomlStatus::AlreadyPresent => {
            println!("{}", tr("cli.init.modelsPresent", &[]))
        }
    }

    match crate::api::cli::builtin_plugins::ensure_builtin_plugins(&cfg)? {
        crate::api::cli::builtin_plugins::BuiltinPluginsStatus::Created => {
            println!("{}", tr("cli.init.pluginInstalled", &[]))
        }
        crate::api::cli::builtin_plugins::BuiltinPluginsStatus::UpdatedExistingPlugin => {
            println!("{}", tr("cli.init.pluginUpdated", &[]))
        }
        crate::api::cli::builtin_plugins::BuiltinPluginsStatus::AlreadyPresent => {
            println!("{}", tr("cli.init.pluginPresent", &[]))
        }
    }

    ensure_embedded_assets(&cfg)?;
    println!("{}", tr("cli.init.assets", &[]));

    match std::env::current_exe() {
        Ok(exe) => {
            if canonicalize(&exe)
                .as_ref()
                .is_ok_and(|path| is_target_deps_artifact(path))
            {
                println!("{}", tr("cli.init.testBinary", &[]));
            } else if let Some(home) = crate::infra::platform::home_dir() {
                let local_bin_dir = user_bin_dir(&home);
                match install_tomcat_command(&exe, &local_bin_dir) {
                    Ok(Some(link)) => println!(
                        "{}",
                        tr(
                            "cli.init.commandLink",
                            &[("path", &crate::infra::platform::format_home_path(&link))]
                        )
                    ),
                    Ok(None) => {}
                    Err(err) => {
                        println!(
                            "{}",
                            tr("cli.init.linkFailed", &[("detail", &err.to_string())])
                        );
                    }
                }
                match add_user_bin_dir_to_path(&home) {
                    Ok(()) => println!(
                        "{}",
                        tr(
                            "cli.init.pathAdded",
                            &[("dir", &local_bin_dir.display().to_string())]
                        )
                    ),
                    Err(step) => {
                        for line in manual_path_lines(&step, false) {
                            println!("{line}");
                        }
                    }
                }
            } else if let Some(bin_dir) = exe.parent() {
                for line in manual_path_lines(&manual_path_step(bin_dir), true) {
                    println!("{line}");
                }
            } else {
                println!("{}", tr("cli.init.binMissing", &[]));
            }
        }
        Err(_) => println!("{}", tr("cli.init.exeMissing", &[])),
    }

    // --- [2/3] 资源检查（与 tomcat doctor 一致，跳过 API Key）---
    println!("{}", tr("cli.init.stageCheck", &[]));
    run_doctor_checks(&cfg, config_file.as_path(), true)?;

    // --- [3/3] API Key 配置 ---
    println!("{}", tr("cli.init.stageKey", &[]));
    let work_dir = get_work_dir(&cfg)?;
    let env_path = work_dir.join("assets").join(".env");
    match crate::api::cli::init_model_wizard::prompt_and_store_provider_key(
        &env_path,
        &model_choice.env_name,
    )? {
        crate::api::cli::init_model_wizard::KeyConfigStatus::AlreadyConfigured => {
            println!(
                "{}",
                tr(
                    "cli.init.keyConfigured",
                    &[("name", &model_choice.env_name)]
                )
            );
        }
        crate::api::cli::init_model_wizard::KeyConfigStatus::Written => {
            println!(
                "{}",
                tr("cli.init.keyWritten", &[("name", &model_choice.env_name)])
            );
        }
        crate::api::cli::init_model_wizard::KeyConfigStatus::Skipped => {
            println!(
                "{}",
                tr(
                    "cli.init.keySkipped",
                    &[
                        ("name", &model_choice.env_name),
                        ("path", &env_path.display().to_string())
                    ]
                )
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
                println!("{}", tr("cli.init.keyConfigured", &[("name", &env_name)]));
            }
            crate::api::cli::init_model_wizard::KeyConfigStatus::Written => {
                println!("{}", tr("cli.init.keyWritten", &[("name", &env_name)]));
            }
            crate::api::cli::init_model_wizard::KeyConfigStatus::Skipped => {
                println!(
                    "{}",
                    tr("cli.init.additionalSkipped", &[("name", &env_name)])
                );
            }
        }
    }

    println!("{}", tr("cli.init.done", &[]));

    Ok(())
}

fn is_target_deps_artifact(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if parent.file_name() != Some(OsStr::new("deps")) {
        return false;
    }
    parent
        .ancestors()
        .any(|ancestor| ancestor.file_name() == Some(OsStr::new("target")))
}

/// 把当前 exe 装成 `local_bin_dir` 里的 `tomcat` 命令；测试二进制、或 exe 本身已在该目录时不装。
pub(crate) fn install_tomcat_command(
    exe: &Path,
    local_bin_dir: &Path,
) -> io::Result<Option<PathBuf>> {
    let exe = canonicalize(exe)?;
    if is_target_deps_artifact(&exe) || exe.starts_with(local_bin_dir) {
        return Ok(None);
    }
    install_user_command(&exe, local_bin_dir, crate::CLI_NAME)
}

/// 自动配置 PATH 做不到时给用户看的步骤；`home_missing` 决定 shell 命令前的那句说明。
pub(crate) fn manual_path_lines(step: &ManualPathStep, home_missing: bool) -> Vec<String> {
    match step {
        ManualPathStep::RunInShell(line) => {
            let key = if home_missing {
                "cli.init.homeMissing"
            } else {
                "cli.init.pathManual"
            };
            vec![tr(key, &[]), format!("    {line}")]
        }
        ManualPathStep::AddUserPathEntry(dir) => vec![tr(
            "cli.init.pathManualWindows",
            &[("dir", &dir.display().to_string())],
        )],
    }
}

/// 与 `tomcat doctor` 相同的逐项检查。`skip_api_key` 为 true 时（用于 `tomcat init` 第二步）不输出 .env 权限与 OPENAI_API_KEY 相关行。
pub(crate) fn run_doctor_checks(
    cfg: &AppConfig,
    config_path: &Path,
    skip_api_key: bool,
) -> Result<(), AppError> {
    if let Err(e) = validate_config(cfg) {
        println!(
            "{}",
            tr("cli.doctor.invalid", &[("detail", &e.to_string())])
        );
        println!(
            "{}",
            tr(
                "cli.doctor.repair",
                &[("path", &config_path.display().to_string())]
            )
        );
        return Ok(());
    }
    if let Err(e) = ensure_work_dir_structure(cfg) {
        println!(
            "{}",
            tr("cli.doctor.workDirFailed", &[("detail", &e.to_string())])
        );
        return Ok(());
    }
    println!(
        "{}",
        tr(
            "cli.doctor.valid",
            &[("path", &config_path.display().to_string())]
        )
    );

    // --- 内嵌资源 ---
    if let Err(e) = ensure_embedded_assets(cfg) {
        println!(
            "{}",
            tr("cli.doctor.assetsFailed", &[("detail", &e.to_string())])
        );
        println!("{}", tr("cli.doctor.assetsHint", &[]));
    } else {
        println!("{}", tr("cli.doctor.assetsReady", &[]));
    }

    // --- rquickjs 运行时 ---
    for line in doctor_plugin_runtime_lines(PluginEngine::global(None).map(|_| ())) {
        println!("{line}");
    }
    for line in doctor_proxy_lines(cfg) {
        println!("{line}");
    }
    if POSIX_SHELL_NEEDS_INSTALL {
        for line in doctor_git_bash_lines(posix_shell().ok()) {
            println!("{line}");
        }
    }

    if !skip_api_key {
        // --- .env 检查 ---
        let work_dir = get_work_dir(cfg)?;
        let env_path = work_dir.join("assets").join(".env");
        if env_path.exists() {
            if let Ok(meta) = std::fs::metadata(&env_path) {
                match mode_bits(&meta).map(|mode| mode & 0o777) {
                    Some(0o600) => println!("{}", tr("cli.doctor.envModeOk", &[])),
                    Some(mode) => {
                        println!(
                            "{}",
                            tr(
                                "cli.doctor.envModeWarning",
                                &[("mode", &format!("{mode:04o}"))]
                            )
                        );
                        println!("  → chmod 600 {}", env_path.display());
                    }
                    None => println!("{}", tr("cli.doctor.envExists", &[])),
                }
            }
        } else {
            println!("{}", tr("cli.doctor.envMissing", &[]));
            println!("{}", tr("cli.doctor.envHint", &[]));
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
            Ok(k) if !k.is_empty() => {
                println!("{}", tr("cli.doctor.keySet", &[("name", &key_env)]))
            }
            _ => {
                println!("{}", tr("cli.doctor.keyMissing", &[("name", &key_env)]));
                println!(
                    "{}",
                    tr(
                        "cli.doctor.keyHint",
                        &[("path", &env_path.display().to_string())]
                    )
                );
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
            lines.push(tr("cli.doctor.proxyConfigured", &[]));
            if raw != trimmed {
                lines.push(tr("cli.doctor.proxyWhitespace", &[]));
            }
        }
        None if !env_proxies.is_empty() => {
            let keys = env_proxies
                .iter()
                .map(|item| item.key)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(tr("cli.doctor.envProxy", &[("keys", &keys)]));
        }
        None => {
            lines.push(tr("cli.doctor.noProxy", &[]));
        }
    }

    for item in env_proxies {
        if item.had_whitespace {
            lines.push(tr("cli.doctor.proxyEnvWhitespace", &[("name", item.key)]));
        }
        if item.key == "ALL_PROXY" && item.value.to_ascii_lowercase().starts_with("socks5://") {
            lines.push(tr("cli.doctor.socksUnsupported", &[]));
        }
    }

    lines
}

pub(crate) fn doctor_plugin_runtime_lines(probe: Result<(), AppError>) -> Vec<String> {
    match probe {
        Ok(()) => vec![tr("cli.doctor.quickJsReady", &[])],
        Err(e) => vec![
            tr("cli.doctor.quickJsFailed", &[("detail", &e.to_string())]),
            tr("cli.doctor.quickJsHint", &[]),
        ],
    }
}

fn doctor_git_bash_lines(git_bash: Option<PathBuf>) -> Vec<String> {
    match git_bash {
        Some(path) => vec![tr(
            "cli.doctor.gitBashReady",
            &[("path", &path.display().to_string())],
        )],
        None => vec![
            tr("cli.doctor.gitBashMissing", &[]),
            tr("bash.gitBashMissing", &[]),
        ],
    }
}

pub(crate) fn run_doctor() -> Result<(), AppError> {
    let path = match normalize_path(DEFAULT_CONFIG_PATH) {
        Ok(p) if p.exists() => p,
        _ => {
            println!("{}", tr("cli.doctor.noConfig", &[]));
            println!("{}", tr("cli.doctor.noConfigHint", &[]));
            return Ok(());
        }
    };
    let cfg = match load_config(Some(path.as_path())) {
        Ok(cfg) => cfg,
        Err(e) => {
            println!(
                "{}",
                tr("cli.doctor.loadFailed", &[("detail", &e.to_string())])
            );
            println!(
                "{}",
                tr(
                    "cli.doctor.repair",
                    &[("path", &path.display().to_string())]
                )
            );
            return Ok(());
        }
    };
    run_doctor_checks(&cfg, path.as_path(), false)?;
    Ok(())
}
