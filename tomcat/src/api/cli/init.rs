//! `tomcat init` 与 `tomcat doctor` 子命令实现。

use crate::infra::i18n::tr;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{
    ensure_embedded_assets, ensure_work_dir_structure, get_work_dir, load_config,
    load_config_for_init, load_store, normalize_path, resolve_sessions_dir, save_store,
    validate_config, write_file_atomic, AppConfig, AppError, PluginEngine, SessionStore,
    DEFAULT_LLM_MODEL,
};

use super::DEFAULT_CONFIG_PATH;

const LOCAL_BIN_EXPORT_LINE: &str = "export PATH=\"$HOME/.local/bin:$PATH\"";
const TOMCAT_INIT_COMMENT: &str = "# Added by tomcat init";

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
        load_config_for_init(Some(&config_file))?
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
            let canonical_exe = std::fs::canonicalize(&exe);
            if canonical_exe
                .as_ref()
                .is_ok_and(|path| is_target_deps_artifact(path))
            {
                println!("{}", tr("cli.init.testBinary", &[]));
            } else if let Some(home) = crate::infra::platform::home_dir() {
                let local_bin_dir = canonical_local_bin_dir(&home);
                match install_canonical_symlink(&exe, &local_bin_dir) {
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
                if auto_add_to_path(&home) {
                    println!("{}", tr("cli.init.pathAdded", &[]));
                } else {
                    println!("{}", tr("cli.init.pathManual", &[]));
                    println!("    {}", LOCAL_BIN_EXPORT_LINE);
                }
            } else if let Some(bin_dir) = exe.parent() {
                println!("{}", tr("cli.init.homeMissing", &[]));
                println!("    export PATH=\"{}:$PATH\"", bin_dir.display());
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

fn canonical_local_bin_dir(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

fn canonical_tomcat_command_path(local_bin_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        local_bin_dir.join("tomcat.exe")
    } else {
        local_bin_dir.join("tomcat")
    }
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

pub(crate) fn install_canonical_symlink(
    exe: &Path,
    local_bin_dir: &Path,
) -> io::Result<Option<PathBuf>> {
    let exe = std::fs::canonicalize(exe)?;
    if is_target_deps_artifact(&exe) || exe.starts_with(local_bin_dir) {
        return Ok(None);
    }

    std::fs::create_dir_all(local_bin_dir)?;
    let link = canonical_tomcat_command_path(local_bin_dir);

    if let Ok(meta) = std::fs::symlink_metadata(&link) {
        if meta.file_type().is_symlink() {
            if std::fs::canonicalize(&link)
                .ok()
                .as_deref()
                .is_some_and(|target| target == exe.as_path())
            {
                return Ok(None);
            }
            std::fs::remove_file(&link)?;
        } else {
            return Ok(None);
        }
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&exe, &link)?;
    }
    #[cfg(windows)]
    {
        std::fs::copy(&exe, &link)?;
    }

    Ok(Some(link))
}

pub(crate) fn path_export_targets(shell: &str, home: &Path) -> Vec<PathBuf> {
    if shell.contains("zsh") {
        return vec![home.join(".zprofile"), home.join(".zshrc")];
    }
    if shell.contains("bash") {
        return vec![home.join(".bashrc")];
    }
    vec![home.join(".profile")]
}

/// 将稳定的 `~/.local/bin` 追加到 shell 启动脚本中的 PATH；已存在同序 export 则跳过。
pub(crate) fn auto_add_to_path(home: &Path) -> bool {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let mut ok = true;
    for profile in path_export_targets(&shell, home) {
        ok &= prune_stale_tomcat_exports(&profile);
        ok &= append_export_once(&profile, LOCAL_BIN_EXPORT_LINE);
    }
    if shell.contains("bash") {
        // macOS 的 login bash 会优先读 .bash_profile（存在时不再继续读 .profile），而
        // 交互式 bash 通常读 .bashrc。为避免截断用户写在 .profile 里的通用环境（如 Rust 的
        // ~/.cargo/env），PATH 仍写进 .bashrc，再确保 .bash_profile 同时 source
        // .profile 与 .bashrc。
        ok &= ensure_bash_profile_sources_profile_and_bashrc(home);
    }
    ok
}

/// 幂等地把一行 export 追加到指定 shell 启动脚本；已存在同序 export 则跳过。
fn append_export_once(profile: &Path, export_line: &str) -> bool {
    if let Ok(content) = std::fs::read_to_string(profile) {
        if content.contains(export_line) {
            return true;
        }
    }
    let mut f = match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(profile)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    writeln!(f, "\n{}\n{}", TOMCAT_INIT_COMMENT, export_line).is_ok()
}

fn is_stale_tomcat_target_export(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("export PATH=") && trimmed.contains("/target/")
}

pub(crate) fn prune_stale_lines(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut result = Vec::with_capacity(lines.len());
    let mut changed = false;
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.trim() == TOMCAT_INIT_COMMENT
            && lines
                .get(index + 1)
                .is_some_and(|next| is_stale_tomcat_target_export(next))
        {
            changed = true;
            index += 2;
            continue;
        }
        if is_stale_tomcat_target_export(line) {
            changed = true;
            index += 1;
            continue;
        }
        result.push(line);
        index += 1;
    }

    if !changed {
        return content.to_string();
    }

    let mut pruned = result.join("\n");
    if content.ends_with('\n') {
        pruned.push('\n');
    }
    pruned
}

pub(crate) fn prune_stale_tomcat_exports(profile: &Path) -> bool {
    let content = match std::fs::read_to_string(profile) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let pruned = prune_stale_lines(&content);
    if pruned == content {
        return true;
    }
    write_file_atomic(profile, pruned.as_bytes()).is_ok()
}

/// 确保 `~/.bash_profile` 会同时 source `~/.profile` 与 `~/.bashrc`，使 macOS 上的
/// login bash 既保留用户已有的通用环境变量，也能加载写入 `.bashrc` 的 PATH。
fn ensure_bash_profile_sources_profile_and_bashrc(home: &Path) -> bool {
    let bash_profile = home.join(".bash_profile");
    let profile_snippet = "[ -r \"$HOME/.profile\" ] && . \"$HOME/.profile\"";
    let bashrc_snippet = "[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"";
    let content = std::fs::read_to_string(&bash_profile).unwrap_or_default();
    let needs_profile = !content.contains(profile_snippet);
    let needs_bashrc = !content.contains(bashrc_snippet);
    if !needs_profile && !needs_bashrc {
        return true;
    }
    let mut f = match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&bash_profile)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    if writeln!(f, "\n# Added by tomcat init").is_err() {
        return false;
    }
    if needs_profile && writeln!(f, "{}", profile_snippet).is_err() {
        return false;
    }
    if needs_bashrc && writeln!(f, "{}", bashrc_snippet).is_err() {
        return false;
    }
    true
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
                        println!("{}", tr("cli.doctor.envModeOk", &[]));
                    } else {
                        println!(
                            "{}",
                            tr(
                                "cli.doctor.envModeWarning",
                                &[("mode", &format!("{mode:04o}"))]
                            )
                        );
                        println!("  → chmod 600 {}", env_path.display());
                    }
                }
            }
            #[cfg(not(unix))]
            println!("{}", tr("cli.doctor.envExists", &[]));
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
