use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use crate::infra::error::AppError;

use super::DEFAULT_SESSION_KEY;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Code,
    Claw,
}

impl SessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Claw => "claw",
        }
    }
}

impl FromStr for SessionMode {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "code" => Ok(Self::Code),
            "claw" => Ok(Self::Claw),
            other => Err(AppError::Config(format!(
                "session.default_mode 非法: {other}（允许 code / claw）"
            ))),
        }
    }
}

pub fn resolve_session_mode(
    default_mode: &str,
    env_override: Option<&str>,
) -> Result<SessionMode, AppError> {
    SessionMode::from_str(env_override.unwrap_or(default_mode))
}

pub fn session_key_for(mode: SessionMode, cwd: &Path) -> String {
    session_key_for_agent("main", mode, cwd)
}

pub fn session_key_for_agent(agent_id: &str, mode: SessionMode, cwd: &Path) -> String {
    if matches!(mode, SessionMode::Claw) && agent_id == "main" {
        return DEFAULT_SESSION_KEY.to_string();
    }

    let channel_key = match mode {
        SessionMode::Claw => "main".to_string(),
        SessionMode::Code => {
            let root = project_root(cwd);
            format!("proj:{}", fnv1a_hex(stable_path_string(&root).as_bytes()))
        }
    };
    format!("agent:{agent_id}:{channel_key}")
}

pub fn project_root(cwd: &Path) -> PathBuf {
    git_toplevel(cwd).unwrap_or_else(|| stable_path(cwd))
}

pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x00000100000001B3);
    }
    format!("{hash:016x}")
}

fn git_toplevel(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8(output.stdout).ok()?;
    let trimmed = root.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(stable_path(Path::new(trimmed)))
}

fn stable_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn stable_path_string(path: &Path) -> String {
    stable_path(path).to_string_lossy().replace('\\', "/")
}
