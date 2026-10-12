//! Stable built-in LLM tool catalog.
//!
//! Prompt caching treats the tool array as the first cacheable prefix. Mode is
//! therefore enforced by each tool handler rather than by adding or removing
//! catalog entries per request. `load_skill` remains policy-controlled because
//! it depends on whether the session exposes any skills.

use serde_json::Value;

/// Generates the stable OpenAI function definition list.
///
/// With the exception of `load_skill`, every built-in tool remains present in
/// every session mode. Tool handlers are the authority on whether a call is
/// allowed for the current runtime state.
///
/// The result uses the same serde shape as `build_function_definitions`:
/// ```json
/// [{ "type": "function", "function": { "name": ..., "description": ..., "parameters": {...} } }]
/// ```
pub fn all_tools_with_policy(allow_load_skill: bool) -> Vec<Value> {
    crate::core::tools::contract::catalog::builtin_tool_surface_with_policy(allow_load_skill)
        .function_definitions
}
