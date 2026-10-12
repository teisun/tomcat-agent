use super::*;

#[test]
fn home_and_cwd_entries_share_one_lock() {
    assert!(std::ptr::eq(home_env_lock(), cwd_lock()));
}
