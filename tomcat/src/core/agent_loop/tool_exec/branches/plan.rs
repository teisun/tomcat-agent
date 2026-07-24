use crate::infra::events::ToolDisplay;

use super::super::{ToolExecCtx, ToolExecOutcome};

pub(in super::super) async fn dispatch_plan_tool(
    ctx: &ToolExecCtx<'_>,
    name: &str,
    args: &serde_json::Value,
    display_out: &mut Option<ToolDisplay>,
) -> ToolExecOutcome {
    let Some(rt) = ctx.plan_runtime else {
        return ToolExecOutcome::err(format!(
            "plan 工具 `{name}` 不可用：当前 AgentLoop 未注入 PlanRuntime（reviewer 子 Agent 或独立测试路径）"
        ));
    };
    if name == "create_plan" && ctx.subagent_type.is_reviewer() {
        return ToolExecOutcome::err(
            "reviewer 子 Agent 禁止调用 `create_plan`（防套娃；reviewer.md §5.2 / §5.5）",
        );
    }

    use crate::core::tools::plan_tool as plan_tools;

    let result: Result<serde_json::Value, plan_tools::ToolError> = match name {
        "create_plan" => {
            match serde_json::from_value::<plan_tools::create_plan::CreatePlanArgs>(args.clone()) {
                Ok(a) => plan_tools::create_plan::execute_with_reviewer(rt, a, true).await,
                Err(e) => Err(plan_tools::ToolError::BadArgs(e.to_string())),
            }
        }
        "update_plan" => {
            match serde_json::from_value::<plan_tools::update_plan::UpdatePlanArgs>(args.clone()) {
                Ok(a) => plan_tools::update_plan::execute_for_tool(rt, a, ctx.tool_call_id).await,
                Err(e) => Err(plan_tools::ToolError::BadArgs(e.to_string())),
            }
        }
        "todos" => match serde_json::from_value::<plan_tools::todos::TodosArgs>(args.clone()) {
            Ok(a) => {
                plan_tools::todos::execute(rt, ctx.todos_runtime.map(|runtime| runtime.as_ref()), a)
            }
            Err(e) => Err(plan_tools::ToolError::BadArgs(e.to_string())),
        },
        "ask_question" => {
            let Some(panel) = rt.ask_question_panel() else {
                return ToolExecOutcome::err(
                    "ask_question 不可用：PlanRuntime 未配置 AskQuestionPanel",
                );
            };
            let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let watcher_flag = cancel_flag.clone();
            let cancel_clone = ctx.cancel.clone();
            let bridge = tokio::spawn(async move {
                cancel_clone.cancelled().await;
                watcher_flag.store(true, std::sync::atomic::Ordering::Release);
            });
            let timeout_ms = rt.ask_question_timeout_ms();
            let res = plan_tools::ask_question::execute_with_timeout(
                rt,
                panel.as_ref(),
                args,
                cancel_flag,
                timeout_ms,
            )
            .await;
            bridge.abort();
            res
        }
        _ => unreachable!("dispatch_plan_tool called with unknown name {name}"),
    };

    match result {
        Ok(v) => {
            *display_out = match name {
                "create_plan" | "update_plan" => v
                    .get("path")
                    .and_then(|value| value.as_str())
                    .map(|plan| ToolDisplay::Plan {
                        plan: plan.to_string(),
                    }),
                _ => None,
            };
            ToolExecOutcome::ok(v.to_string())
        }
        Err(e) => ToolExecOutcome::err(format!("{name} 失败：{e}")),
    }
}
