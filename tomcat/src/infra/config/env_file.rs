use std::collections::BTreeMap;
use std::path::Path;

use crate::infra::error::AppError;
use crate::infra::i18n::tr;
use crate::infra::platform::write_file_atomic;

pub(crate) fn env_file_error(path: &Path, error: dotenvy::Error) -> AppError {
    // dotenvy's line/variable variants carry raw credential text. Never render it.
    let detail = match error {
        dotenvy::Error::LineParse(_, position) => {
            tr("config.envLine", &[("position", &position.to_string())])
        }
        dotenvy::Error::Io(error) => error.to_string(),
        dotenvy::Error::EnvVar(_) => tr("config.envVarInvalid", &[]),
        _ => tr("config.envInvalid", &[]),
    };
    AppError::Config(tr(
        "config.parseFile",
        &[("path", &path.display().to_string()), ("detail", &detail)],
    ))
}

pub fn read_env_entries(env_path: &Path) -> Result<BTreeMap<String, String>, AppError> {
    let mut vars = BTreeMap::new();
    if !env_path.exists() {
        return Ok(vars);
    }
    let iter =
        dotenvy::from_path_iter(env_path).map_err(|error| env_file_error(env_path, error))?;
    for entry in iter {
        let (key, value) = entry.map_err(|error| env_file_error(env_path, error))?;
        if !key.trim().is_empty() {
            vars.insert(key, value);
        }
    }
    Ok(vars)
}

pub fn write_env_entries(env_path: &Path, vars: &BTreeMap<String, String>) -> Result<(), AppError> {
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }

    let mut lines = vec![tr("config.envHeader", &[])];
    for (key, value) in vars.iter().filter(|(key, _)| !is_proxy_key(key)) {
        lines.push(format!("{key}={value}"));
    }
    lines.push(String::new());
    lines.push(tr("config.envProxyHint", &[]));
    for key in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
        match vars.get(key) {
            Some(value) => lines.push(format!("{key}={value}")),
            None => lines.push(format!("# {}={}", key, proxy_placeholder(key))),
        }
    }
    write_file_atomic(env_path, format!("{}\n", lines.join("\n")).as_bytes())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(env_path, perms).map_err(AppError::Io)?;
    }

    Ok(())
}

fn is_proxy_key(key: &str) -> bool {
    matches!(key, "HTTPS_PROXY" | "HTTP_PROXY" | "ALL_PROXY")
}

fn proxy_placeholder(key: &str) -> &'static str {
    match key {
        "ALL_PROXY" => "socks5://127.0.0.1:7890",
        _ => "http://127.0.0.1:7890",
    }
}
