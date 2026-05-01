pub(super) fn build_simple_diff(old: &str, new: &str) -> String {
    let o: Vec<&str> = old.lines().collect();
    let n: Vec<&str> = new.lines().collect();
    let mut out = String::new();
    for (i, (a, b)) in o.iter().zip(n.iter()).enumerate() {
        if a != b {
            out.push_str(&format!("  {} -{}\n  {} +{}\n", i + 1, a, i + 1, b));
        }
    }
    if o.len() != n.len() {
        out.push_str(&format!("  ... ({} -> {} lines)\n", o.len(), n.len()));
    }
    if out.is_empty() {
        out = "(无变化)".to_string();
    }
    out
}
