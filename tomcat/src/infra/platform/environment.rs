//! Immutable child-process environment received from a controlling connection.
//! Never install it into the service process, persist it, or print its values.
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::infra::error::AppError;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentSnapshot(BTreeMap<String, String>);

impl std::fmt::Debug for EnvironmentSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentSnapshot")
            .field("variables", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl EnvironmentSnapshot {
    pub fn capture() -> Result<Self, AppError> {
        let values = std::env::vars_os()
            .map(|(key, value)| {
                Ok((
                    key.into_string().map_err(|_| invalid_environment())?,
                    value.into_string().map_err(|_| invalid_environment())?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, AppError>>()?;
        Self::from_values(values)
    }

    pub fn from_values(values: BTreeMap<String, String>) -> Result<Self, AppError> {
        let snapshot = Self(values);
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if self
            .0
            .iter()
            .any(|(key, value)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0'))
        {
            return Err(invalid_environment());
        }
        #[cfg(windows)]
        {
            let mut keys = std::collections::HashSet::new();
            if self
                .0
                .keys()
                .any(|key| !keys.insert(key.to_ascii_uppercase()))
            {
                return Err(invalid_environment());
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        #[cfg(windows)]
        return self
            .0
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str());
        #[cfg(not(windows))]
        self.0.get(key).map(String::as_str)
    }

    pub fn apply(&self, command: &mut std::process::Command) {
        command.env_clear().envs(&self.0);
    }

    /// Resolve Windows PATH/PATHEXT from the actual child environment, never
    /// falling back to CreateProcess's search through the service's PATH.
    pub fn resolve_program(
        &self,
        command: &str,
        cwd: &std::path::Path,
        overrides: &BTreeMap<String, String>,
    ) -> Result<PathBuf, AppError> {
        let overrides = Self::from_values(overrides.clone())?;
        let program = PathBuf::from(command);
        if program.is_absolute() {
            return Ok(program);
        }
        if program.components().count() > 1 {
            return Ok(cwd.join(program));
        }
        #[cfg(windows)]
        {
            let path = overrides
                .get("PATH")
                .or_else(|| self.get("PATH"))
                .unwrap_or("");
            let directories = std::env::split_paths(path)
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(|dir| cwd.join(dir))
                .collect::<Vec<_>>();
            if !directories.is_empty() {
                let path = std::env::join_paths(directories).map_err(|_| invalid_environment())?;
                let pathext = overrides.get("PATHEXT").or_else(|| self.get("PATHEXT"));
                if let Some(found) = super::executable::find_executable_in(
                    &path,
                    pathext.map(std::ffi::OsStr::new),
                    &[command],
                    std::path::Path::is_file,
                ) {
                    return Ok(found);
                }
            }
            Err(AppError::Config(
                "child program not found in the controlling connection's PATH".into(),
            ))
        }
        #[cfg(not(windows))]
        {
            let _ = overrides;
            // exec searches the child PATH after applying its environment and cwd.
            Ok(program)
        }
    }

    /// Bash path checks must expand ~ using the same HOME as the child, not the
    /// service's HOME. A directory argument still does not expand arbitrary $VAR.
    pub fn expand_home(&self, path: &str) -> Result<PathBuf, AppError> {
        let home_relative =
            path == "~" || path.starts_with("~/") || (cfg!(windows) && path.starts_with("~\\"));
        if !home_relative {
            return Ok(PathBuf::from(path));
        }
        let home = self.get("HOME");
        #[cfg(windows)]
        let home = home.or_else(|| self.get("USERPROFILE"));
        let home = home.filter(|value| !value.is_empty()).ok_or_else(|| {
            AppError::Primitive(
                "bash path uses ~ but the controlling connection has no home directory".into(),
            )
        })?;
        let rest = path
            .strip_prefix("~/")
            .or({
                #[cfg(windows)]
                {
                    path.strip_prefix("~\\")
                }
                #[cfg(not(windows))]
                {
                    None
                }
            })
            .unwrap_or("");
        Ok(PathBuf::from(home).join(rest))
    }
}

fn invalid_environment() -> AppError {
    AppError::Config("invalid connection environment: expected UTF-8 names and values without NUL, empty names, or '=' in names".into())
}

#[cfg(test)]
#[path = "tests/environment_test.rs"]
mod tests;
