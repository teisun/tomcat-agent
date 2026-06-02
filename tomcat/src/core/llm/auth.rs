use crate::infra::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub provider: String,
    pub env_name: String,
    pub value: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AuthStore;

impl AuthStore {
    pub fn get(&self, provider: &str, fallback_env: Option<&str>) -> Result<Credential, AppError> {
        let inferred_env = env_name_for_provider(provider);
        if let Some(value) = read_env_value(&inferred_env) {
            return Ok(Credential {
                provider: provider.trim().to_string(),
                env_name: inferred_env,
                value,
            });
        }

        if let Some(fallback_env) = fallback_env
            .map(str::trim)
            .filter(|env| !env.is_empty() && *env != inferred_env)
        {
            if let Some(value) = read_env_value(fallback_env) {
                return Ok(Credential {
                    provider: provider.trim().to_string(),
                    env_name: fallback_env.to_string(),
                    value,
                });
            }
        }

        Err(AppError::Config(missing_key_message(
            provider,
            &inferred_env,
            fallback_env,
        )))
    }
}

pub fn env_name_for_provider(provider: &str) -> String {
    let normalized = provider
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{}_API_KEY", normalized)
}

pub fn missing_key_message(
    provider: &str,
    inferred_env: &str,
    fallback_env: Option<&str>,
) -> String {
    match fallback_env
        .map(str::trim)
        .filter(|env| !env.is_empty() && *env != inferred_env)
    {
        Some(fallback) => format!(
            "未找到 provider=`{}` 的凭证，请设置 `{}`（或兼容回退 `{}`）。",
            provider.trim(),
            inferred_env,
            fallback
        ),
        None => format!(
            "未找到 provider=`{}` 的凭证，请设置 `{}`。",
            provider.trim(),
            inferred_env
        ),
    }
}

fn read_env_value(env_name: &str) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
