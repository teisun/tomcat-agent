use std::collections::BTreeMap;
use std::path::Path;

use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic;

pub fn read_env_entries(env_path: &Path) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    if !env_path.exists() {
        return vars;
    }
    if let Ok(iter) = dotenvy::from_path_iter(env_path) {
        for (key, value) in iter.flatten() {
            if !key.trim().is_empty() {
                vars.insert(key, value);
            }
        }
    }
    vars
}

pub fn write_env_entries(
    env_path: &Path,
    vars: &BTreeMap<String, String>,
) -> Result<(), AppError> {
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }

    let mut lines =
        vec!["# tomcat runtime credentials — 此文件由 tomcat init 生成，权限 0600".to_string()];
    for (key, value) in vars.iter().filter(|(key, _)| !is_proxy_key(key)) {
        lines.push(format!("{key}={value}"));
    }
    lines.push(String::new());
    lines.push("# 如需通过代理访问大模型，取消以下注释并填入代理地址：".to_string());
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
