use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use super::store::CheckpointStore;
use super::types::{
    CheckpointDiff, CheckpointError, CheckpointId, CheckpointMeta, CheckpointRecordRequest,
    CheckpointRestoreReport, ListOptions, RestoreOptions, RetentionPolicy,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WORKDIR_FILES: usize = 50_000;
const TOMCAT_WORKDIR_MARKER: &str = "TOMCAT_WORKDIR";
const METADATA_FILE: &str = "metadata.json";
const REF_PREFIX: &str = "refs/tomcat-checkpoints";

#[derive(Debug)]
pub struct ShadowGitStore {
    checkpoints_root: PathBuf,
    git_dir: PathBuf,
    work_tree: PathBuf,
    metadata_path: PathBuf,
    workdir_marker_path: PathBuf,
    max_workdir_files: usize,
    lock: Mutex<()>,
}

impl ShadowGitStore {
    pub fn new(agent_trail_dir: PathBuf, work_tree: PathBuf) -> Self {
        let work_tree = canonicalize_existing_path(work_tree);
        let checkpoints_root = agent_trail_dir.join("checkpoints");
        let hash = workdir_hash(&work_tree);
        let git_dir = checkpoints_root.join(hash);
        let metadata_path = git_dir.join(METADATA_FILE);
        let workdir_marker_path = git_dir.join(TOMCAT_WORKDIR_MARKER);
        Self {
            checkpoints_root,
            git_dir,
            work_tree,
            metadata_path,
            workdir_marker_path,
            max_workdir_files: MAX_WORKDIR_FILES,
            lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    fn with_max_workdir_files(
        agent_trail_dir: PathBuf,
        work_tree: PathBuf,
        max_workdir_files: usize,
    ) -> Self {
        let mut store = Self::new(agent_trail_dir, work_tree);
        store.max_workdir_files = max_workdir_files;
        store
    }

    fn ensure_repo_initialized(&self) -> Result<(), CheckpointError> {
        fs::create_dir_all(&self.checkpoints_root)?;
        if self.git_dir.join("HEAD").exists() {
            self.ensure_metadata_file()?;
            self.write_workdir_marker()?;
            return Ok(());
        }

        let output = Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(&self.git_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        if !output.status.success() {
            return Err(CheckpointError::CommandFailed(output_message(&output)));
        }

        self.run_git(["config", "core.bare", "false"])?;
        self.run_git([
            "config",
            "core.worktree",
            self.work_tree.to_string_lossy().as_ref(),
        ])?;
        self.run_git(["config", "user.name", "tomcat-checkpoint"])?;
        self.run_git(["config", "user.email", "tomcat@local"])?;
        self.run_git(["config", "commit.gpgsign", "false"])?;
        self.run_git(["config", "tag.gpgsign", "false"])?;
        self.ensure_metadata_file()?;
        self.write_workdir_marker()?;
        Ok(())
    }

    fn ensure_metadata_file(&self) -> Result<(), CheckpointError> {
        if !self.metadata_path.exists() {
            fs::write(&self.metadata_path, b"[]")?;
        }
        Ok(())
    }

    fn write_workdir_marker(&self) -> Result<(), CheckpointError> {
        fs::write(
            &self.workdir_marker_path,
            self.work_tree.to_string_lossy().as_bytes(),
        )?;
        Ok(())
    }

    fn load_metadata(&self) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        self.ensure_metadata_file()?;
        let content = fs::read_to_string(&self.metadata_path)?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(trimmed)?)
    }

    fn save_metadata(&self, entries: &[CheckpointMeta]) -> Result<(), CheckpointError> {
        let content = serde_json::to_string_pretty(entries)?;
        fs::write(&self.metadata_path, content)?;
        Ok(())
    }

    fn run_git<I, S>(&self, args: I) -> Result<Output, CheckpointError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run_git_allow_failure(args)?;
        if output.status.success() {
            return Ok(output);
        }
        Err(CheckpointError::CommandFailed(output_message(&output)))
    }

    fn run_git_allow_failure<I, S>(&self, args: I) -> Result<Output, CheckpointError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new("git");
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", null_device())
            .env("GIT_CONFIG_GLOBAL", null_device());
        cmd.args(args);

        let mut child = cmd.spawn()?;
        let started = Instant::now();
        loop {
            if child.try_wait()?.is_some() {
                return child.wait_with_output().map_err(CheckpointError::Io);
            }
            if started.elapsed() >= GIT_TIMEOUT {
                let _ = child.kill();
                let output = child.wait_with_output()?;
                return Err(CheckpointError::CommandFailed(format!(
                    "git command timed out after {}s: {}",
                    GIT_TIMEOUT.as_secs(),
                    output_message(&output)
                )));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn git_status_has_changes(&self) -> Result<bool, CheckpointError> {
        let output = self.run_git([
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignored=no",
        ])?;
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    }

    fn current_head_commit(&self) -> Result<Option<String>, CheckpointError> {
        let output = self.run_git_allow_failure(["rev-parse", "--verify", "HEAD"])?;
        if !output.status.success() {
            return Ok(None);
        }
        let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if commit.is_empty() {
            Ok(None)
        } else {
            Ok(Some(commit))
        }
    }

    fn make_meta(
        &self,
        id: CheckpointId,
        request: &CheckpointRecordRequest,
        git_commit: Option<String>,
    ) -> CheckpointMeta {
        CheckpointMeta {
            id,
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            kind: request.kind.clone(),
            git_commit,
            message_anchor: request.message_anchor.clone(),
            created_at: Utc::now().to_rfc3339(),
            notes: request.notes.clone(),
        }
    }

    fn new_checkpoint_id() -> CheckpointId {
        CheckpointId::new(format!(
            "ck_{}_{}",
            Utc::now().timestamp_millis(),
            short_hash(Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64)
        ))
    }

    fn save_checkpoint_meta(
        &self,
        checkpoint_id: CheckpointId,
        request: &CheckpointRecordRequest,
        head_commit: Option<String>,
    ) -> Result<CheckpointId, CheckpointError> {
        let meta = self.make_meta(checkpoint_id.clone(), request, head_commit.clone());
        let mut metas = self.load_metadata()?;
        metas.push(meta);
        self.save_metadata(&metas)?;
        if let Some(commit) = head_commit {
            let ref_name = format!("{}/{}", REF_PREFIX, checkpoint_id.as_str());
            self.run_git(["update-ref", &ref_name, &commit])?;
        }
        Ok(checkpoint_id)
    }

    fn commit_index_as_checkpoint(
        &self,
        request: &CheckpointRecordRequest,
        previous_head: Option<String>,
        allow_empty: bool,
    ) -> Result<CheckpointId, CheckpointError> {
        self.run_git(["add", "-A"])?;
        let checkpoint_id = Self::new_checkpoint_id();
        let message = format!("checkpoint {}", checkpoint_id);
        if allow_empty {
            self.run_git(["commit", "--allow-empty", "-m", &message])?;
        } else {
            self.run_git(["commit", "-m", &message])?;
        }
        let head_commit = self.current_head_commit()?.or(previous_head);
        self.save_checkpoint_meta(checkpoint_id, request, head_commit)
    }

    fn validate_restore_paths(&self, paths: &[PathBuf]) -> Result<Vec<PathBuf>, CheckpointError> {
        let mut out = Vec::with_capacity(paths.len());
        for path in paths {
            if path.is_absolute() {
                return Err(CheckpointError::InvalidPath(format!(
                    "restore path must be relative: {}",
                    path.display()
                )));
            }
            let mut cleaned = PathBuf::new();
            for component in path.components() {
                match component {
                    Component::CurDir => {}
                    Component::Normal(part) => cleaned.push(part),
                    Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                        return Err(CheckpointError::InvalidPath(format!(
                            "restore path escapes workspace: {}",
                            path.display()
                        )));
                    }
                }
            }
            if cleaned.as_os_str().is_empty() {
                return Err(CheckpointError::InvalidPath(
                    "restore path cannot be empty".to_string(),
                ));
            }
            out.push(cleaned);
        }
        Ok(out)
    }

    fn pathspecs_for_restore(&self, paths: &[PathBuf]) -> Result<Vec<PathBuf>, CheckpointError> {
        if paths.is_empty() {
            return Ok(vec![PathBuf::from(".")]);
        }
        self.validate_restore_paths(paths)
    }

    fn diff_for_paths(
        &self,
        commit: &str,
        paths: &[PathBuf],
    ) -> Result<CheckpointDiff, CheckpointError> {
        let pathspecs = self.pathspecs_for_restore(paths)?;
        let mut diff_args = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            "--stat".to_string(),
            commit.to_string(),
            "--".to_string(),
        ];
        for path in &pathspecs {
            diff_args.push(path.to_string_lossy().to_string());
        }
        let diff_out = self.run_git(diff_args.iter().map(|s| s.as_str()))?;

        let mut names_args = vec![
            "diff".to_string(),
            "--name-only".to_string(),
            commit.to_string(),
            "--".to_string(),
        ];
        for path in &pathspecs {
            names_args.push(path.to_string_lossy().to_string());
        }
        let names_out = self.run_git(names_args.iter().map(|s| s.as_str()))?;
        let changed_paths = String::from_utf8_lossy(&names_out.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(PathBuf::from)
            .collect();

        Ok(CheckpointDiff {
            text: String::from_utf8_lossy(&diff_out.stdout).to_string(),
            changed_paths,
        })
    }

    fn prune_orphan_repositories(&self) -> Result<usize, CheckpointError> {
        let mut removed = 0;
        if !self.checkpoints_root.exists() {
            return Ok(0);
        }
        for entry in fs::read_dir(&self.checkpoints_root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || path == self.git_dir {
                continue;
            }
            let marker = path.join(TOMCAT_WORKDIR_MARKER);
            if !marker.exists() {
                continue;
            }
            let workdir = fs::read_to_string(&marker)?;
            if !Path::new(workdir.trim()).exists() {
                fs::remove_dir_all(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl CheckpointStore for ShadowGitStore {
    fn record(&self, request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        let _guard = self.lock.lock();
        self.ensure_repo_initialized()?;

        let existing = self
            .load_metadata()?
            .into_iter()
            .find(|meta| {
                meta.session_id == request.session_id
                    && meta.turn_id == request.turn_id
                    && meta.kind == request.kind
            })
            .map(|meta| meta.id);
        if let Some(id) = existing {
            return Ok(id);
        }

        if count_regular_files_until(&self.work_tree, self.max_workdir_files + 1)?
            > self.max_workdir_files
        {
            return Ok(CheckpointId::null());
        }

        let previous_head = self.current_head_commit()?;
        if !self.git_status_has_changes()? {
            let metas = self.load_metadata()?;
            if let Some(id) = metas.last().map(|meta| meta.id.clone()) {
                return Ok(id);
            }
            if previous_head.is_none() {
                return self.commit_index_as_checkpoint(&request, None, true);
            }
            return self.save_checkpoint_meta(Self::new_checkpoint_id(), &request, previous_head);
        }

        self.commit_index_as_checkpoint(&request, previous_head, false)
    }

    fn list(
        &self,
        session_id: &str,
        opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        let _guard = self.lock.lock();
        self.ensure_repo_initialized()?;
        let mut entries: Vec<CheckpointMeta> = self
            .load_metadata()?
            .into_iter()
            .filter(|meta| meta.session_id == session_id)
            .collect();
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(limit) = opts.limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }

    fn show(&self, id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        let _guard = self.lock.lock();
        self.ensure_repo_initialized()?;
        Ok(self
            .load_metadata()?
            .into_iter()
            .find(|meta| &meta.id == id))
    }

    fn diff(&self, id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        let _guard = self.lock.lock();
        self.ensure_repo_initialized()?;
        let meta = self
            .load_metadata()?
            .into_iter()
            .find(|meta| &meta.id == id)
            .ok_or_else(|| CheckpointError::NotFound(id.to_string()))?;
        let commit = meta
            .git_commit
            .ok_or_else(|| CheckpointError::NotFound(id.to_string()))?;
        self.diff_for_paths(&commit, &[])
    }

    fn restore(
        &self,
        id: &CheckpointId,
        opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        let _guard = self.lock.lock();
        self.ensure_repo_initialized()?;
        let meta = self
            .load_metadata()?
            .into_iter()
            .find(|meta| &meta.id == id)
            .ok_or_else(|| CheckpointError::NotFound(id.to_string()))?;
        let commit = meta
            .git_commit
            .clone()
            .ok_or_else(|| CheckpointError::NotFound(id.to_string()))?;
        let diff = self.diff_for_paths(&commit, &opts.paths)?;
        if !opts.dry_run {
            let pathspecs = self.pathspecs_for_restore(&opts.paths)?;
            let mut args = vec!["checkout".to_string(), commit.clone(), "--".to_string()];
            for path in &pathspecs {
                args.push(path.to_string_lossy().to_string());
            }
            self.run_git(args.iter().map(|s| s.as_str()))?;
        }
        Ok(CheckpointRestoreReport {
            checkpoint_id: meta.id,
            changed_paths: diff.changed_paths,
            dry_run: opts.dry_run,
            summary: if diff.text.trim().is_empty() {
                None
            } else {
                Some(diff.text)
            },
        })
    }

    fn prune(&self, retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        let _guard = self.lock.lock();
        let mut removed = self.prune_orphan_repositories()?;
        if !self.git_dir.exists() {
            return Ok(removed);
        }
        self.ensure_repo_initialized()?;
        let metas = self.load_metadata()?;
        if metas.is_empty() {
            return Ok(removed);
        }

        let cutoff = Utc::now() - chrono::Duration::days(retention.retention_days as i64);
        let mut sorted = metas.clone();
        sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let keep_ids: std::collections::HashSet<String> = sorted
            .into_iter()
            .enumerate()
            .filter_map(|(idx, meta)| {
                let created = DateTime::parse_from_rfc3339(&meta.created_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok();
                let within_days = created.map(|dt| dt >= cutoff).unwrap_or(true);
                if idx < retention.retention_max && within_days {
                    Some(meta.id.to_string())
                } else {
                    None
                }
            })
            .collect();

        let mut kept = Vec::new();
        let mut dropped = Vec::new();
        for meta in metas {
            if keep_ids.contains(meta.id.as_str()) {
                kept.push(meta);
            } else {
                dropped.push(meta);
            }
        }

        if dropped.is_empty() {
            return Ok(removed);
        }

        for meta in &dropped {
            let ref_name = format!("{}/{}", REF_PREFIX, meta.id.as_str());
            let _ = self.run_git_allow_failure(["update-ref", "-d", &ref_name]);
        }
        self.save_metadata(&kept)?;
        let _ = self.run_git_allow_failure(["reflog", "expire", "--expire=now", "--all"]);
        let _ = self.run_git_allow_failure(["gc", "--prune=now", "--quiet"]);
        removed += dropped.len();
        Ok(removed)
    }
}

fn null_device() -> &'static str {
    #[cfg(windows)]
    {
        "NUL"
    }
    #[cfg(not(windows))]
    {
        "/dev/null"
    }
}

fn workdir_hash(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn canonicalize_existing_path(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn short_hash(seed: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.to_string().as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(3)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn output_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn count_regular_files_until(path: &Path, max: usize) -> Result<usize, CheckpointError> {
    let mut count = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if count > max {
            break;
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() || file_type.is_symlink() {
                count += 1;
                if count > max {
                    return Ok(count);
                }
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::checkpoint::store::CheckpointStore;
    use crate::core::checkpoint::types::{CheckpointKind, CheckpointRecordRequest, ListOptions};

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn request(turn_id: &str) -> CheckpointRecordRequest {
        CheckpointRecordRequest {
            session_id: "s1".to_string(),
            turn_id: turn_id.to_string(),
            kind: CheckpointKind::TurnEnd,
            message_anchor: Some(format!("msg-{turn_id}")),
            notes: None,
        }
    }

    #[test]
    fn record_and_list_round_trip() {
        if !git_available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("workspace");
        let trail = root.path().join("trail");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&trail).unwrap();
        fs::write(worktree.join("note.txt"), "hello").unwrap();

        let store = ShadowGitStore::new(trail, worktree);
        let id = store.record(request("t1")).unwrap();
        assert!(!id.is_null(), "首次 record 应生成真实 checkpoint");

        let listed = store
            .list("s1", ListOptions { limit: None })
            .expect("list should succeed");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert!(listed[0].git_commit.is_some());

        let shown = store.show(&id).unwrap().expect("show should return meta");
        assert_eq!(shown.turn_id, "t1");
        assert_eq!(shown.message_anchor.as_deref(), Some("msg-t1"));
    }

    #[test]
    fn record_dedups_same_turn_and_kind() {
        if !git_available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("workspace");
        let trail = root.path().join("trail");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&trail).unwrap();
        fs::write(worktree.join("a.txt"), "first").unwrap();

        let store = ShadowGitStore::new(trail, worktree.clone());
        let first = store.record(request("t1")).unwrap();
        fs::write(worktree.join("a.txt"), "second").unwrap();
        let second = store.record(request("t1")).unwrap();
        assert_eq!(first, second, "同 turn + kind 应走 dedup");
        let listed = store.list("s1", ListOptions::default()).unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn record_reuses_latest_checkpoint_when_worktree_unchanged() {
        if !git_available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("workspace");
        let trail = root.path().join("trail");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&trail).unwrap();
        fs::write(worktree.join("a.txt"), "first").unwrap();

        let store = ShadowGitStore::new(trail, worktree);
        let first = store.record(request("t1")).unwrap();
        let second = store.record(request("t2")).unwrap();
        assert_eq!(first, second, "无 diff 时应复用最新 checkpoint");
        let listed = store.list("s1", ListOptions::default()).unwrap();
        assert_eq!(listed.len(), 1, "无 diff 不应生成空提交");
    }

    #[test]
    fn record_first_no_diff_empty_worktree_creates_baseline_checkpoint() {
        if !git_available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("workspace");
        let trail = root.path().join("trail");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&trail).unwrap();

        let store = ShadowGitStore::new(trail, worktree);
        let id = store.record(request("t1")).unwrap();
        assert!(
            !id.is_null(),
            "空工作区首次 record 也应生成 baseline checkpoint"
        );
        let listed = store.list("s1", ListOptions::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert!(listed[0].git_commit.is_some());
    }

    #[test]
    fn record_returns_null_when_worktree_exceeds_limit() {
        if !git_available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("workspace");
        let trail = root.path().join("trail");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&trail).unwrap();
        fs::write(worktree.join("a.txt"), "1").unwrap();
        fs::write(worktree.join("b.txt"), "2").unwrap();

        let store = ShadowGitStore::with_max_workdir_files(trail, worktree, 1);
        let id = store.record(request("t1")).unwrap();
        assert!(id.is_null(), "文件数超阈值时应跳过 checkpoint");
    }

    #[test]
    fn new_canonicalizes_existing_worktree_path() {
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("workspace");
        fs::create_dir_all(&worktree).unwrap();

        let store = ShadowGitStore::new(root.path().join("trail"), worktree.clone());
        assert_eq!(
            store.work_tree,
            fs::canonicalize(worktree).unwrap(),
            "已存在目录应规范化，避免同一路径别名落到不同 checkpoint 仓"
        );
    }
}
