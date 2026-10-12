use sha2::{Digest, Sha256};

const MAX_TOOL_NAME_LEN: usize = 64;

pub fn to_model_name(server: &str, raw_tool: &str) -> String {
    let server_part = sanitize_component(server);
    let tool_part = sanitize_component(raw_tool);
    let readable = format!("mcp__{server_part}__{tool_part}");
    // Preserve normal names, but do not let lossy replacement or the separator
    // collapse different (server, tool) identities into the same short name.
    let unambiguous = server_part == server
        && tool_part == raw_tool
        && !server.contains("__")
        && !raw_tool.contains("__")
        && !server.ends_with('_')
        && !raw_tool.starts_with('_');
    if readable.len() <= MAX_TOOL_NAME_LEN && unambiguous {
        return readable;
    }

    let hash = short_hash(&format!("{server}\0{raw_tool}"));
    let suffix = format!("_{hash}");
    let keep = MAX_TOOL_NAME_LEN.saturating_sub(suffix.len());
    format!("{}{}", truncate_ascii(&readable, keep), suffix)
}

fn sanitize_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unnamed".to_string()
    } else {
        output
    }
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn truncate_ascii(value: &str, len: usize) -> &str {
    &value[..value
        .char_indices()
        .nth(len)
        .map_or(value.len(), |(index, _)| index)]
}

#[cfg(test)]
#[path = "tests/naming_test.rs"]
mod tests;
