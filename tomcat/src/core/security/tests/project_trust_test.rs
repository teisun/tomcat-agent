use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Barrier};

use crate::core::security::project_trust::ProjectTrustStore;
use crate::AppConfig;

fn store(dir: &Path) -> ProjectTrustStore {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.join("user").to_string_lossy().into_owned());
    ProjectTrustStore::open(&cfg).unwrap()
}

#[test]
fn absent_file_idempotence_and_corruption_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let trust = store(temp.path());
    assert!(!trust.is_trusted(&root).unwrap());
    trust.trust(&root).unwrap();
    let first = std::fs::read(trust.path()).unwrap();
    trust.trust(&root).unwrap();
    assert_eq!(std::fs::read(trust.path()).unwrap(), first);
    assert!(store(temp.path()).is_trusted(&root).unwrap());
    std::fs::write(trust.path(), b"not json").unwrap();
    assert!(trust.is_trusted(&root).is_err());
    assert!(trust.trust(&root).is_err());
    assert_eq!(std::fs::read(trust.path()).unwrap(), b"not json");
}

#[test]
fn concurrent_writers_keep_both_projects() {
    let temp = tempfile::tempdir().unwrap();
    let roots = [temp.path().join("a"), temp.path().join("b")];
    for path in &roots {
        std::fs::create_dir(path).unwrap();
    }
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = roots
        .iter()
        .map(|path| {
            let barrier = Arc::clone(&barrier);
            let store = store(temp.path());
            let root = std::fs::canonicalize(path).unwrap();
            std::thread::spawn(move || {
                barrier.wait();
                store.trust(&root).unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let store = store(temp.path());
    for path in roots {
        assert!(store
            .is_trusted(&std::fs::canonicalize(path).unwrap())
            .unwrap());
    }
}

#[test]
fn only_canonical_roots_can_be_trusted() {
    let temp = tempfile::tempdir().unwrap();
    let project = std::fs::canonicalize(temp.path()).unwrap();
    let child = project.join("child");
    std::fs::create_dir(&child).unwrap();
    let store = store(temp.path());
    assert!(ProjectTrustStore::validate_root(Path::new("relative")).is_err());
    assert!(ProjectTrustStore::validate_root(&project.join("missing")).is_err());
    assert!(
        store.trust(&child).is_ok(),
        "non-Git child is its own project"
    );
    assert!(!store.is_trusted(&project).unwrap());
    #[cfg(unix)]
    {
        let link = project.join("alias");
        std::os::unix::fs::symlink(&child, &link).unwrap();
        assert_eq!(ProjectTrustStore::root_for(&link).unwrap(), child);
        assert!(ProjectTrustStore::validate_root(&link).is_err());
    }
}

#[test]
fn git_subfolders_share_one_root_but_other_repos_do_not() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let other = temp.path().join("other");
    for dir in [&repo, &other] {
        std::fs::create_dir(dir).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(dir)
            .status()
            .unwrap()
            .success());
    }
    let nested = repo.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let root = ProjectTrustStore::root_for(&nested).unwrap();
    assert_eq!(root, std::fs::canonicalize(&repo).unwrap());
    let store = store(temp.path());
    assert!(store.trust(&nested).is_err());
    store.trust(&root).unwrap();
    assert!(!store
        .is_trusted(&std::fs::canonicalize(other).unwrap())
        .unwrap());
}
