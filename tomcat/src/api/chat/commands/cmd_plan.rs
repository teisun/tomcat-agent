//! `/plan` 本地斜杠命令（plan-runtime.md §4.1 R1）。
//!
//! 三个子命令均在 chat 层处理，**不**进 LLM、**不**入 tool catalog：
//!
//! ```text
//! /plan                      → PlanRuntime::enter_planning  → mode=Planning
//! /plan exit                 → PlanRuntime::exit_to_chat   → mode=Chat
//! /plan build [plan_id/path] → PlanRuntime::build_plan     → mode=Executing { plan_id }
//! ```
//!
//! P1 只闭环 `enter_planning` / `exit_to_chat` 两条；`build` 在 P6 接入
//! （P1 阶段 `build` 命中会返回结构化提示「P6 落地」）。

use crate::api::chat::ChatContext;

use super::parse::{ChatCommand, ChatCommandOutcome};

/// `/plan` 子命令解析结果（仅在 chat 层使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanCommand {
    /// `/plan`，进入 Planning。
    Enter,
    /// `/plan exit`，退回 Chat。
    Exit,
    /// `/plan build [plan_id/path]`，进入 EXEC（省略参数时走 runtime 默认源）。
    Build { plan_target: Option<String> },
    /// J2：`/plan list`，列出 `~/.tomcat/plans/` 下所有 plan 文件的 id / state / goal。
    List,
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Plan(PlanCommand::Enter),
        [_cmd, sub] if sub == "exit" => ChatCommand::Plan(PlanCommand::Exit),
        [_cmd, sub] if sub == "list" => ChatCommand::Plan(PlanCommand::List),
        [_cmd, sub] if sub == "build" => {
            ChatCommand::Plan(PlanCommand::Build { plan_target: None })
        }
        [_cmd, sub, plan_target] if sub == "build" => ChatCommand::Plan(PlanCommand::Build {
            plan_target: Some(plan_target.clone()),
        }),
        _ => ChatCommand::UsageError {
            message: usage_text(),
        },
    }
}

fn usage_text() -> String {
    "用法错误：/plan | /plan exit | /plan build [plan_id/path] | /plan list".to_string()
}

/// `/plan` 子命令分发。`ctx.plan_runtime` 在 P1 起由 `ChatContext::from_config` 注入。
pub(crate) fn run(ctx: &ChatContext, cmd: PlanCommand) -> ChatCommandOutcome {
    let rt = ctx.plan_runtime.clone();
    match cmd {
        PlanCommand::Enter => match rt.enter_planning() {
            Ok(()) => {
                println!(
                    "[plan] 已进入 PLAN 模式。\n[plan] 先与模型讨论目标；用 /plan exit 退回 Chat；用 /plan build <plan_id/path> 进入 EXEC。"
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
        PlanCommand::Build { plan_target } => {
            let session_id_for_plan = match ctx.session.current_session_id() {
                Ok(Some(v)) => Some(v),
                Ok(None) => {
                    eprintln!("[plan] /plan build 失败：当前无会话，无法确定 session_id");
                    return ChatCommandOutcome::Handled;
                }
                Err(e) => {
                    eprintln!("[plan] /plan build 失败：读取当前 session_id 失败：{}", e);
                    return ChatCommandOutcome::Handled;
                }
            };
            let build_target = match plan_target {
                Some(target) => target,
                None => match rt.default_build_target() {
                    Ok(target) => target,
                    Err(e) => {
                        eprintln!("[plan] /plan build 失败：{}", e);
                        return ChatCommandOutcome::Handled;
                    }
                },
            };
            match rt.build_plan(&build_target, session_id_for_plan) {
                Ok(outcome) => {
                    for w in &outcome.warnings {
                        eprintln!("[plan] warning: {w}");
                    }
                    return ChatCommandOutcome::Continue {
                        line: format!("start building {}", outcome.plan_path.to_string_lossy()),
                        echo_user: true,
                    };
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
/// 输出格式：`<plan_id>  <state>  <goal_first_line>  (updated <iso>)`
/// 找不到目录 / 无文件 → 友好提示，不报错。
fn print_plan_list() {
    use crate::core::plan_runtime::file_store;
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
            plan.frontmatter.state.as_str().to_string(),
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
