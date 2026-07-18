use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use super::store::CheckpointStore;
use super::types::{
    CheckpointDiff, CheckpointError, CheckpointId, CheckpointMeta, CheckpointRecordRequest,
    CheckpointRestoreReport, ListOptions, RestoreOptions, RetentionPolicy,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const CHECKPOINT_COOLDOWN: Duration = Duration::from_secs(60);
const MAX_WORKDIR_FILES: usize = 50_000;
const TOMCAT_WORKDIR_MARKER: &str = "TOMCAT_WORKDIR";
const METADATA_FILE: &str = "metadata.json";
const REF_PREFIX: &str = "refs/tomcat-checkpoints";
const DEFAULT_GIT_PROGRAM: &str = "git";
const EXCLUDE_MARKER: &str = "# tomcat checkpoint default excludes";
const DEFAULT_EXCLUDE_RULES: &[&str] = &[
    "node_modules/",
    "dist/",
    "build/",
    "target/",
    "out/",
    ".next/",
    ".nuxt/",
    ".vite/",
    "__pycache__/",
    "*.pyc",
    "*.pyo",
    ".cache/",
    ".pytest_cache/",
    ".mypy_cache/",
    ".ruff_cache/",
    "coverage/",
    ".coverage",
    ".venv/",
    "venv/",
    "env/",
    ".git/",
    ".hg/",
    ".svn/",
    ".worktrees/",
    "*.log",
    ".DS_Store",
    "Thumbs.db",
    "*.zip",
    "*.tar",
    "*.tar.gz",
    "*.tgz",
    "*.7z",
    "*.rar",
    "*.iso",
    ".env",
    ".env.*",
    ".env.local",
    ".env.*.local",
    "third_party/",
];

#[derive(Debug)]
pub struct ShadowGitStore {
    checkpoints_root: PathBuf,
    git_dir: PathBuf,
    work_tree: PathBuf,
    metadata_path: PathBuf,
    workdir_marker_path: PathBuf,
    max_workdir_files: usize,
    git_timeout: Duration,
    git_program: PathBuf,
    cooldown_until: Mutex<Option<Instant>>,
    lock: Mutex<()>,
    #[cfg(test)]
    file_count_calls: AtomicUsize,
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
            git_timeout: GIT_TIMEOUT,
            git_program: PathBuf::from(DEFAULT_GIT_PROGRAM),
            cooldown_until: Mutex::new(None),
            lock: Mutex::new(()),
            #[cfg(test)]
            file_count_calls: AtomicUsize::new(0),
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

    #[cfg(test)]
    fn with_git_timeout(mut self, git_timeout: Duration) -> Self {
        store_set_git_timeout(&mut self, git_timeout);
        self
    }

    #[cfg(test)]
    fn with_git_program(mut self, git_program: PathBuf) -> Self {
        self.git_program = git_program;
        self
    }

    #[cfg(test)]
    fn force_cooldown_until(&self, until: Instant) {
        *self.cooldown_until.lock() = Some(until);
    }

    #[cfg(test)]
    fn cooldown_until(&self) -> Option<Instant> {
        *self.cooldown_until.lock()
    }

    #[cfg(test)]
    fn file_count_calls(&self) -> usize {
        self.file_count_calls.load(Ordering::SeqCst)
    }

    fn ensure_repo_initialized(&self) -> Result<(), CheckpointError> {
        fs::create_dir_all(&self.checkpoints_root)?;
        if self.git_dir.join("HEAD").exists() {
            self.ensure_metadata_file()?;
            self.write_workdir_marker()?;
            self.ensure_exclude_file()?;
            return Ok(());
        }

        let output = Command::new(&self.git_program)
            .arg("init")
            .arg("--bare")
            .arg(&self.git_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        if !output.status.success() {
            return Err(CheckpointError::CommandFailed(summarize_git_failure(
                "git init",
                &self.work_tree,
                None,
                &output,
            )));
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
        self.ensure_exclude_file()?;
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

    fn ensure_exclude_file(&self) -> Result<(), CheckpointError> {
        let exclude_path = self.git_dir.join("info").join("exclude");
        let existing = match fs::read_to_string(&exclude_path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err.into()),
        };
        let mut lines: Vec<String> = if existing.is_empty() {
            Vec::new()
        } else {
            existing.lines().map(str::to_string).collect()
        };
        let mut changed = false;

        if !existing.contains(EXCLUDE_MARKER) {
            if lines.last().is_some_and(|line| !line.is_empty()) {
                lines.push(String::new());
            }
            lines.push(EXCLUDE_MARKER.to_string());
            changed = true;
        }

        for rule in DEFAULT_EXCLUDE_RULES {
            if !existing.lines().any(|line| line.trim() == *rule) {
                lines.push((*rule).to_string());
                changed = true;
            }
        }

        if changed {
            if let Some(parent) = exclude_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut content = lines.join("\n");
            if !content.ends_with('\n') {
                content.push('\n');
            }
            fs::write(exclude_path, content)?;
        }
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
        let args = collect_git_args(args);
        let output = self.run_git_allow_failure_args(&args)?;
        if output.status.success() {
            return Ok(output);
        }
        Err(CheckpointError::CommandFailed(summarize_git_failure(
            &describe_git_command(&args),
            &self.work_tree,
            None,
            &output,
        )))
    }

    fn run_git_allow_failure<I, S>(&self, args: I) -> Result<Output, CheckpointError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect_git_args(args);
        self.run_git_allow_failure_args(&args)
    }

    fn run_git_allow_failure_args(&self, args: &[OsString]) -> Result<Output, CheckpointError> {
        let mut cmd = Command::new(&self.git_program);
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
            if started.elapsed() >= self.git_timeout {
                let _ = child.kill();
                let output = child.wait_with_output()?;
                return Err(CheckpointError::CommandTimedOut(summarize_git_failure(
                    &describe_git_command(args),
                    &self.work_tree,
                    Some(self.git_timeout),
                    &output,
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

    fn staged_changed_paths(&self) -> Result<Vec<String>, CheckpointError> {
        let output = self.run_git_allow_failure(["diff", "--cached", "--name-only", "--"])?;
        if !output.status.success() {
            return Err(CheckpointError::CommandFailed(summarize_git_failure(
                "git diff --cached --name-only",
                &self.work_tree,
                None,
                &output,
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect())
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

    fn count_worktree_files_until(&self, max: usize) -> Result<usize, CheckpointError> {
        #[cfg(test)]
        self.file_count_calls.fetch_add(1, Ordering::SeqCst);
        let output = self.run_git([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])?;
        let mut count = 0usize;
        for entry in output.stdout.split(|byte| *byte == b'\0') {
            if entry.is_empty() {
                continue;
            }
            count += 1;
            if count > max {
                return Ok(count);
            }
        }
        Ok(count)
    }

    fn make_meta(
        &self,
        id: CheckpointId,
        request: &CheckpointRecordRequest,
        git_commit: Option<String>,
        notes: Option<serde_json::Value>,
    ) -> CheckpointMeta {
        CheckpointMeta {
            id,
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            kind: request.kind.clone(),
            git_commit,
            message_anchor: request.message_anchor.clone(),
            created_at: Utc::now().to_rfc3339(),
            notes,
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
        changed_paths: &[String],
    ) -> Result<CheckpointId, CheckpointError> {
        let meta = self.make_meta(
            checkpoint_id.clone(),
            request,
            head_commit.clone(),
            merge_notes_with_changed_paths(request.notes.as_ref(), changed_paths),
        );
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
        let changed_paths = self.staged_changed_paths()?;
        let checkpoint_id = Self::new_checkpoint_id();
        let message = format!("checkpoint {}", checkpoint_id);
        if allow_empty {
            self.run_git(["commit", "--allow-empty", "-m", &message])?;
        } else {
            self.run_git(["commit", "-m", &message])?;
        }
        let head_commit = self.current_head_commit()?.or(previous_head);
        self.save_checkpoint_meta(checkpoint_id, request, head_commit, &changed_paths)
    }

    fn record_impl(
        &self,
        request: CheckpointRecordRequest,
    ) -> Result<CheckpointId, CheckpointError> {
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

        if self.count_worktree_files_until(self.max_workdir_files + 1)? > self.max_workdir_files {
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
            return self.save_checkpoint_meta(
                Self::new_checkpoint_id(),
                &request,
                previous_head,
                &[],
            );
        }

        self.commit_index_as_checkpoint(&request, previous_head, false)
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
        if matches!(
            cooldown_decision(Instant::now(), *self.cooldown_until.lock()),
            CooldownDecision::Skip
        ) {
            return Ok(CheckpointId::null());
        }

        match self.record_impl(request) {
            Ok(id) => {
                *self.cooldown_until.lock() = None;
                Ok(id)
            }
            Err(err) if err.is_timeout() => {
                *self.cooldown_until.lock() =
                    Some(next_cooldown(Instant::now(), CHECKPOINT_COOLDOWN));
                Err(err)
            }
            Err(err) => Err(err),
        }
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

fn merge_notes_with_changed_paths(
    notes: Option<&serde_json::Value>,
    changed_paths: &[String],
) -> Option<serde_json::Value> {
    if changed_paths.is_empty() {
        return notes.cloned();
    }

    let mut object = match notes.cloned() {
        Some(serde_json::Value::Object(map)) => map,
        Some(other) => {
            let mut map = serde_json::Map::new();
            map.insert("sourceNotes".to_string(), other);
            map
        }
        None => serde_json::Map::new(),
    };
    object.insert(
        "changedPaths".to_string(),
        serde_json::Value::Array(
            changed_paths
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    Some(serde_json::Value::Object(object))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CooldownDecision {
    Skip,
    Proceed,
}

fn cooldown_decision(now: Instant, until: Option<Instant>) -> CooldownDecision {
    match until {
        Some(until) if now < until => CooldownDecision::Skip,
        _ => CooldownDecision::Proceed,
    }
}

fn next_cooldown(now: Instant, duration: Duration) -> Instant {
    now + duration
}

fn collect_git_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect()
}

fn describe_git_command(args: &[OsString]) -> String {
    match args.first() {
        Some(cmd) => format!("git {}", cmd.to_string_lossy()),
        None => "git".to_string(),
    }
}

fn summarize_git_failure(
    command: &str,
    work_tree: &Path,
    timeout: Option<Duration>,
    output: &Output,
) -> String {
    let captured_bytes = output.stdout.len() + output.stderr.len();
    let omitted = if captured_bytes == 0 {
        "captured output omitted".to_string()
    } else {
        format!("captured output omitted {captured_bytes} bytes")
    };
    match timeout {
        Some(timeout) => format!(
            "{command} timed out after {} (work_tree={}, {omitted})",
            render_duration(timeout),
            work_tree.display()
        ),
        None => format!(
            "{command} failed ({}; work_tree={}, {omitted})",
            render_exit_status(&output.status),
            work_tree.display()
        ),
    }
}

fn render_exit_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_string(),
    }
}

fn render_duration(duration: Duration) -> String {
    if duration.as_millis() < 1_000 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{}s", duration.as_secs())
    }
}

#[cfg(test)]
fn store_set_git_timeout(store: &mut ShadowGitStore, git_timeout: Duration) {
    store.git_timeout = git_timeout;
}

#[cfg(test)]
#[path = "tests/shadow_git_test.rs"]
mod tests;
