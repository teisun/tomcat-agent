use super::super::input::build_readline_config;

#[test]
fn build_readline_config_enables_bracketed_paste() {
    let cfg = build_readline_config();
    assert!(cfg.enable_bracketed_paste());
}
