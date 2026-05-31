use super::super::{ToolExecCtx, AGENT_PLUGIN_ID};

pub(in super::super) async fn handle_bash(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let command = args["command"].as_str().unwrap_or("");
    let cwd = args["cwd"].as_str();
    let argv_store: Option<Vec<String>> = args.get("args").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    });
    let argv_ref = argv_store.as_deref();
    let timeout_ms_override: Option<u64> = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(crate::infra::MAX_TOOLS_BASH_TIMEOUT_MS));
    let run_in_background = args
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if run_in_background {
        super::handle_bash_background(
            ctx.bash_task_registry,
            ctx.subagent_type,
            command,
            cwd,
            argv_store,
        )
        .await
    } else {
        ctx.primitive
            .execute_bash(command, cwd, AGENT_PLUGIN_ID, argv_ref, timeout_ms_override)
            .await
            .map(|r| {
                let mut out = String::new();
                if !r.stdout.is_empty() {
                    out.push_str(&r.stdout);
                }
                if !r.stderr.is_empty() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str("STDERR: ");
                    out.push_str(&r.stderr);
                }
                out.push_str(&format!("\n(exit code: {})", r.exit_code));
                out
            })
            .map_err(|e| e.to_string())
    }
}
