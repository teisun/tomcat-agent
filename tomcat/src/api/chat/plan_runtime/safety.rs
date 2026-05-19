//! `plan_id` 安全校验（plan §P0.5 / §8 D-? 路径穿越防御）。

use super::PlanRuntimeError;

/// 校验 `plan_id` 仅含 `a-z 0-9 _ -` 字符，且不为空。
///
/// 失败：
/// - 空串 → `UnsafePlanId("empty")`
/// - 含 `/`、`\\`、`..`、控制字符或空白 → `UnsafePlanId("forbidden char(s)")`
/// - 含其它非 ASCII 字符 → `UnsafePlanId("non-ascii")`
///
/// 目的：`~/.tomcat/plans/<plan_id>.plan.md` 必须落在 `plans/` 子树内，**不**允许 `../etc/passwd` /
/// `subdir/secret` / 控制字符 / 大写盘符等导致路径穿越或 Windows 大小写歧义。
pub fn assert_plan_id_safe(plan_id: &str) -> Result<(), PlanRuntimeError> {
    if plan_id.is_empty() {
        return Err(PlanRuntimeError::UnsafePlanId("empty".into()));
    }
    // 显式拒：路径分隔 / 父引用 / 控制字符 / 空白
    for ch in plan_id.chars() {
        if ch == '/' || ch == '\\' || ch.is_control() || ch.is_whitespace() {
            return Err(PlanRuntimeError::UnsafePlanId(format!(
                "forbidden char {ch:?}"
            )));
        }
    }
    if plan_id.contains("..") {
        return Err(PlanRuntimeError::UnsafePlanId("contains ..".into()));
    }
    if !plan_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(PlanRuntimeError::UnsafePlanId(format!(
            "non-[a-z0-9_-] chars in {plan_id:?}"
        )));
    }
    Ok(())
}

/// 在 plan_id 落盘前调用的「最后一道防线」：与 `assert_plan_id_safe` 等价，但语义化命名以便
/// 在 `file_store::write_plan` 等位置形成自文档化的 grep 锚点。
pub fn assert_plan_id_safe_for_disk(plan_id: &str) -> Result<(), PlanRuntimeError> {
    assert_plan_id_safe(plan_id)
}
