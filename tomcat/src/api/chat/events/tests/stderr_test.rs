use super::{accepts_session, should_render_preflight_ui};
use crate::infra::event_bus::EventContext;
use serde_json::json;

#[test]
fn search_tools_preflight_ui_is_hidden_when_disabled() {
    assert!(!should_render_preflight_ui(false, "ready"));
    assert!(!should_render_preflight_ui(false, "failed"));
}

#[test]
fn search_tools_preflight_ui_is_shown_when_enabled() {
    assert!(should_render_preflight_ui(true, "ready"));
    assert!(should_render_preflight_ui(true, "failed"));
}

#[test]
fn git_preflight_ui_is_hidden_when_disabled() {
    assert!(!should_render_preflight_ui(false, "start"));
    assert!(!should_render_preflight_ui(false, "already_installing"));
}

#[test]
fn git_preflight_ui_is_shown_when_enabled() {
    assert!(should_render_preflight_ui(true, "start"));
    assert!(should_render_preflight_ui(true, "already_installing"));
}

#[test]
fn accepts_session_allows_all_when_unbound() {
    let evt = EventContext::new("ev", json!({})).with_session_id("s1");
    assert!(accepts_session(None, &evt));
}

#[test]
fn accepts_session_requires_exact_bound_session_match() {
    let evt = EventContext::new("ev", json!({})).with_session_id("s1");
    assert!(accepts_session(Some("s1"), &evt));
    assert!(!accepts_session(Some("s2"), &evt));
}

#[test]
fn accepts_session_rejects_global_event_for_session_bound_listener() {
    let evt = EventContext::new("ev", json!({}));
    assert!(!accepts_session(Some("s1"), &evt));
}
