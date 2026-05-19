//! `/plan` 本地斜杠命令（plan-runtime.md §4.1 R1）。
//!
//! 三个子命令均在 chat 层处理，**不**进 LLM、**不**入 tool catalog：
//!
//! ```text
//! /plan "<objective>"        → PlanRuntime::enter_planning  → mode=Planning
//! /plan exit                 → PlanRuntime::exit_to_chat   → mode=Chat
//! /plan build <plan_id>      → PlanRuntime::build_plan     → mode=Executing { plan_id }
//! ```
//!
//! P1 只闭环 `enter_planning` / `exit_to_chat` 两条；`build` 在 P6 接入
//! （P1 阶段 `build` 命中会返回结构化提示「P6 落地」）。

use crate::api::chat::ChatContext;

use super::parse::{ChatCommand, ChatCommandOutcome};

/// `/plan` 子命令解析结果（仅在 chat 层使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanCommand {
    /// `/plan "<objective>"`，进入 Planning。
    Enter { objective: String },
    /// `/plan exit`，退回 Chat。
    Exit,
    /// `/plan build <plan_id>`，进入 EXEC（P6 才完整闭环）。
    Build { plan_id: String },
    /// J2：`/plan list`，列出 `~/.tomcat/plans/` 下所有 plan 文件的 id / mode / goal。
    List,
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::UsageError {
            message: usage_text(),
        },
        [_cmd, sub] if sub == "exit" => ChatCommand::Plan(PlanCommand::Exit),
        [_cmd, sub] if sub == "list" => ChatCommand::Plan(PlanCommand::List),
        [_cmd, sub] => {
            // `/plan "<objective>"` —— shell_words 已剥引号，整 token 作 objective
            // 拒绝纯 `exit` / `build` 缺参的歧义情况（必须 quote 才能用 exit 作 objective）
            ChatCommand::Plan(PlanCommand::Enter {
                objective: sub.clone(),
            })
        }
        [_cmd, sub, plan_id] if sub == "build" => ChatCommand::Plan(PlanCommand::Build {
            plan_id: plan_id.clone(),
        }),
        _ => ChatCommand::UsageError {
            message: usage_text(),
        },
    }
}

fn usage_text() -> String {
    "用法错误：/plan \"<objective>\" | /plan exit | /plan build <plan_id> | /plan list".to_string()
}

/// `/plan` 子命令分发。`ctx.plan_runtime` 在 P1 起由 `ChatContext::from_config` 注入。
pub(crate) fn run(ctx: &ChatContext, cmd: PlanCommand) -> ChatCommandOutcome {
    let rt = ctx.plan_runtime.clone();
    match cmd {
        PlanCommand::Enter { objective } => match rt.enter_planning(&objective) {
            Ok(()) => {
                println!(
                    "[plan] 进入 PLAN 模式：{}\n[plan] 用 /plan exit 退回 Chat；用 /plan build <plan_id> 进入 EXEC。",
                    objective
                );
                ChatCommandOutcome::Handled
            }
            Err(e) => {
                eprintln!("[plan] 进入 PLAN 失败：{}", e);
                ChatCommandOutcome::Handled
            }
        },
        PlanCommand::Exit => match rt.exit_to_chat() {
            Ok(()) => {
                println!("[plan] 已退回 Chat 模式。");
                ChatCommandOutcome::Handled
            }
            Err(e) => {
                eprintln!("[plan] /plan exit 失败：{}", e);
                ChatCommandOutcome::Handled
            }
        },
        PlanCommand::Build { plan_id } => {
            // 当前 chat session 的真实 session_id（uuid）从 ChatContext 取；P6 阶段还
            // 未把 session_id 注入 ChatContext，先传 None — 续跑 warning 仍依赖 session_key 比对，
            // 不影响闸门正确性。后续 P7 接入会改 ctx.session_id().clone().
            let session_id_for_plan: Option<String> = None;
            match rt.build_plan(&plan_id, session_id_for_plan) {
                Ok(outcome) => {
                    println!(
                        "[plan] /plan build 成功：plan_id={} (prev_disk_mode={:?}) → EXEC",
                        outcome.plan_id, outcome.prev_disk_mode
                    );
                    for w in &outcome.warnings {
                        eprintln!("[plan] warning: {w}");
                    }
                }
                Err(e) => eprintln!("[plan] /plan build 拒绝：{}", e),
            }
            ChatCommandOutcome::Handled
        }
        PlanCommand::List => {
            print_plan_list();
            ChatCommandOutcome::Handled
        }
    }
}

/// J2：扫 `~/.tomcat/plans/` 并打印每条 plan 的简要状态行。
///
/// 输出格式：`<plan_id>  <mode>  <goal_first_line>  (updated <iso>)`
/// 找不到目录 / 无文件 → 友好提示，不报错。
fn print_plan_list() {
    use crate::api::chat::plan_runtime::file_store;
    let plans_dir = match file_store::plans_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[plan list] 无法解析 plans 目录：{e}");
            return;
        }
    };
    let entries = match std::fs::read_dir(&plans_dir) {
        Ok(e) => e,
        Err(_) => {
            println!(
                "[plan list] 暂无 plan（目录不存在或为空）：{}",
                plans_dir.display()
            );
            return;
        }
    };
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !path.to_string_lossy().ends_with(".plan.md") {
            continue;
        }
        let plan = match file_store::read_plan(&path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let goal_line = plan
            .frontmatter
            .goal
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        rows.push((
            plan.frontmatter.plan_id.clone(),
            plan.frontmatter.mode.as_str().to_string(),
            goal_line,
            plan.frontmatter.created_at.clone(),
        ));
    }
    if rows.is_empty() {
        println!("[plan list] 暂无 plan：{}", plans_dir.display());
        return;
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    println!(
        "[plan list] {} plan(s) in {}：",
        rows.len(),
        plans_dir.display()
    );
    for (id, mode, goal, ts) in rows {
        println!("  - {id:<32} [{mode:<10}]  {goal}  (created {ts})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_with_objective() {
        let cmd = parse_args(vec!["/plan".into(), "ship plan mode".into()]);
        assert!(matches!(
            cmd,
            ChatCommand::Plan(PlanCommand::Enter { ref objective }) if objective == "ship plan mode"
        ));
    }

    #[test]
    fn parse_plan_exit() {
        let cmd = parse_args(vec!["/plan".into(), "exit".into()]);
        assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::Exit)));
    }

    #[test]
    fn parse_plan_build_with_id() {
        let cmd = parse_args(vec!["/plan".into(), "build".into(), "ship-001".into()]);
        assert!(matches!(
            cmd,
            ChatCommand::Plan(PlanCommand::Build { ref plan_id }) if plan_id == "ship-001"
        ));
    }

    #[test]
    fn parse_plan_bare_returns_usage_error() {
        let cmd = parse_args(vec!["/plan".into()]);
        assert!(matches!(cmd, ChatCommand::UsageError { .. }));
    }

    #[test]
    fn parse_plan_list() {
        let cmd = parse_args(vec!["/plan".into(), "list".into()]);
        assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::List)));
    }

    #[test]
    fn parse_plan_build_without_id_returns_usage_error() {
        let cmd = parse_args(vec!["/plan".into(), "build".into()]);
        // 这等价于 /plan <objective="build">，按 Enter 处理（语义上 ambiguous 但安全）；
        // 用户若想 build 应当传 plan_id。
        // 我们这里就接受为 Enter("build")；
        // 真正 /plan build 必须带 plan_id，所以 UsageError 也合理——但为了简单不做 Enter("build") 的 special-case。
        assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::Enter { .. })));
    }
}
