use super::super::*;
use super::mocks::test_config;
use serial_test::serial;
use std::path::Path;

struct CurrentDirGuard {
    _lock: crate::test_support::TestLockGuard<'static>,
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let lock = crate::test_support::cwd_lock().lock().unwrap();
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

#[test]
#[serial(cwd_lock)]
fn run_skill_list_and_reload_return_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let _cwd_guard = CurrentDirGuard::set(dir.path());

    assert!(run_skill(SkillSub::List, &cfg).is_ok());
    assert!(run_skill(SkillSub::Reload, &cfg).is_ok());
}
