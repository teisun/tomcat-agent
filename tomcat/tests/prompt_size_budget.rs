//! Prompt 静态体积测量 + 瘦身门槛（净 token 为负的量化兜底）。
//!
//! 两个用途：
//! 1. `print_prompt_static_sizes`（`--nocapture` 时打印）：产出优化前后 chars 对照表的原始数据。
//! 2. `chat_tool_defs_below_baseline` / `full_tool_defs_below_baseline`：把优化后的静态体积钉在
//!    基线之下，防止未来回退。基线常量取自"优化前"实测（见 BASELINE_* 注释）。

use tomcat::core::llm::system_prompt::build_system_prompt;
use tomcat::core::plan_runtime::catalog::all_tools_with_policy;
use tomcat::core::prompts::{load, PromptKey};
use tomcat::core::tools::contract::catalog::{
    build_function_definitions, render_tool_guidelines_with_policy, BUILTIN_TOOL_CATALOG,
};

fn serialized_len(defs: &[serde_json::Value]) -> usize {
    defs.iter()
        .map(|d| serde_json::to_string(d).unwrap().chars().count())
        .sum()
}

const SYSTEM_TEMPLATES: &[(&str, PromptKey)] = &[
    ("core_identity", PromptKey::SystemCoreIdentity),
    ("tool_instructions", PromptKey::SystemToolInstructions),
    ("output_conventions", PromptKey::SystemOutputConventions),
    ("parallel_tools", PromptKey::SystemParallelTools),
    ("paged_reading", PromptKey::SystemPagedReading),
    (
        "background_shell_monitor",
        PromptKey::SystemBackgroundShellMonitor,
    ),
    ("verification", PromptKey::SystemVerification),
    ("available_skills", PromptKey::SystemAvailableSkills),
    ("workspace_context", PromptKey::SystemWorkspaceContext),
    ("workspace_state", PromptKey::SystemWorkspaceState),
];

// Current accepted contract baseline (chars). Keep one character above the latest measured
// footprint so an unreviewed prompt-growth regression fails this test.
const BASELINE_CHAT_TOOLDEFS: usize = 33_198;
// v5 adds the declared acceptance-command contract and ratcheted green-build evidence to the
// plan tools (32,004 -> 33,197; 36,549 -> 37,742). The chat and full catalogs share that schema
// growth; keep one character above each measured footprint as a strict ceiling.
const BASELINE_FULL_TOOLDEFS: usize = 37_743;

#[test]
fn print_prompt_static_sizes() {
    println!("\n=== SYSTEM TEMPLATES (chars) ===");
    let mut sys_total = 0usize;
    for (name, key) in SYSTEM_TEMPLATES {
        let n = load(*key).chars().count();
        sys_total += n;
        println!("  {name:<26} {n:>6}");
    }
    println!("  {:<26} {:>6}", "SYS TOTAL", sys_total);

    println!("\n=== PER-TOOL description (chars) ===");
    let mut desc_total = 0usize;
    for entry in BUILTIN_TOOL_CATALOG {
        let n = entry.description.chars().count();
        desc_total += n;
        println!("  {:<16} {:>6}", entry.name, n);
    }
    println!("  {:<16} {:>6}", "DESC TOTAL", desc_total);

    let full = build_function_definitions();
    let chat = all_tools_with_policy(false);
    println!("\n=== TOOL DEFS serialized (chars) ===");
    println!(
        "  full  build_function_definitions : {}",
        serialized_len(&full)
    );
    println!(
        "  stable all_tools_with_policy(false): {}",
        serialized_len(&chat)
    );

    let guidelines = render_tool_guidelines_with_policy(true);
    let tool_instr_rendered = load(PromptKey::SystemToolInstructions)
        .replace("{tool_guidelines}", &guidelines)
        .chars()
        .count();
    let assembled = build_system_prompt("/Users/yan/proj").chars().count();
    println!("\n=== ASSEMBLED (chars) ===");
    println!(
        "  aggregated tool_guidelines        : {}",
        guidelines.chars().count()
    );
    println!("  tool_instructions rendered        : {tool_instr_rendered}");
    println!("  build_system_prompt (default)     : {assembled}");
    println!("=== END ===\n");
}

#[test]
fn chat_tool_defs_below_baseline() {
    let chat_len = serialized_len(&all_tools_with_policy(false));
    assert!(
        chat_len < BASELINE_CHAT_TOOLDEFS,
        "CHAT tool defs must shrink below baseline: got {chat_len}, baseline {BASELINE_CHAT_TOOLDEFS}"
    );
}

#[test]
fn full_tool_defs_below_baseline() {
    let full_len = serialized_len(&build_function_definitions());
    assert!(
        full_len < BASELINE_FULL_TOOLDEFS,
        "full tool defs must shrink below baseline: got {full_len}, baseline {BASELINE_FULL_TOOLDEFS}"
    );
}
