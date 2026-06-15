use crate::infra::net_guard::{
    validate_http_url, HttpSchemePolicy, UrlValidationOptions, ValidatedHttpUrl,
};
use crate::infra::AppError;

use super::types::MAX_URL_LENGTH;

pub(crate) type ValidatedUrl = ValidatedHttpUrl;

/// 校验模型直接传入的 `url`，并在首跳前把 `http` 升为 `https`。
pub(crate) fn validate_input_url(raw: &str) -> Result<ValidatedUrl, AppError> {
    validate_http_url(
        raw,
        UrlValidationOptions {
            max_url_length: MAX_URL_LENGTH,
            error_prefix: "web_fetch",
            scheme_policy: HttpSchemePolicy::UpgradeToHttps,
        },
    )
}

/// 校验重定向目标；redirect 场景不做 `http -> https` 自动升级。
pub(crate) fn validate_redirect_url(raw: &str) -> Result<ValidatedUrl, AppError> {
    validate_http_url(
        raw,
        UrlValidationOptions {
            max_url_length: MAX_URL_LENGTH,
            error_prefix: "web_fetch",
            scheme_policy: HttpSchemePolicy::AllowHttp,
        },
    )
}
