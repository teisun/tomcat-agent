//! T2-P0-016 PR-I（bash T2 后台）：起后台任务 + task_output / task_stop / task_list。
//!
//! ## 与 PR-E `bash` 同步路径的差异
//!
//! - **同步 bash**：`spawn → 等 wait → 收齐输出 → 一次返回 BashResult`，单轮 tool 阻塞。
//! - **后台 bash**（本模块）：`bash` 工具带 `run_in_background=true` → **立即**返回
//!   `BashTaskTicket{ task_id, log_path }`；后台 `tokio::spawn` 守护把 stdout/stderr
//!   持续写到 `<persist_dir>/bash-<task_id>.log`；模型用三件套自驱：
//!     - `task_output`：按字节偏移拉日志增量；
//!     - `task_stop`：`killpg(SIGKILL)` 杀整组（与 PR-E.2 同口径）；
//!     - `task_list`：枚举所有 task 现状（含 `Finished{ exit_code }` / `Stopped`）。
//!
//! ## 锁分层（避免「stop 等 wait」死锁）
//!
//! - `BashTaskRegistry.tasks: RwLock<HashMap<...>>`：注册表本身，操作短促。
//! - `BashTask.info: RwLock<BashTaskInfo>`：每任务的元信息 + 状态机，操作短促。
//! - 子进程 `Child` 句柄**不**入锁——直接 move 进 wait 任务（独占 `await`）。
//!   stop 走的是 `pid → libc::killpg(SIGKILL)`，不依赖 Child 句柄，杀完
//!   wait 任务自然 `wait()` 返回 → 状态翻成 `Finished{ exit_code }`。

use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;

use crate::infra::error::AppError;

/// 任务唯一 ID（`<unix_ms>-<rand6>`，避免 `uuid` 依赖）。
pub type BashTaskId = String;

/// `bash` 后台任务的状态机：`Running` → (`Stopped` | `Finished { exit_code }`).
///
/// `Stopped` 由 `task_stop` 主动触发；其后 wait 任务感知到 `child.wait()`
/// 返回也**不**回退覆盖（避免「人为 stop」被覆盖成「自然 Finished」误判）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BashTaskStatus {
    Running,
    Stopped,
    Finished { exit_code: i32 },
}

/// `task_list` 返回的单条快照；同时也是 `BashTaskRegistry::spawn` 内部的元信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashTaskInfo {
    pub task_id: BashTaskId,
    pub command: String,
    pub started_at_unix_ms: u128,
    pub log_path: String,
    pub status: BashTaskStatus,
}

/// `bash run_in_background=true` 的回执：模型只拿到 `task_id` + `log_path`，
/// 不阻塞当前 tool 轮次。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashTaskTicket {
    pub task_id: BashTaskId,
    pub log_path: String,
    pub started_at_unix_ms: u128,
}

/// `task_output` 返回的增量：`content` 是 `[start_offset, next_offset)`
/// 字节窗口的 UTF-8 lossy 解码；模型下次传 `since=next_offset` 拉续读。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashTaskOutputChunk {
    pub task_id: BashTaskId,
    pub content: String,
    pub start_offset: u64,
    pub next_offset: u64,
    pub finished: bool,
    pub exit_code: Option<i32>,
}

struct BashTask {
    info: RwLock<BashTaskInfo>,
    /// 子进程 PID（spawn 后立即记录）；stop 路径 `libc::killpg(pid, SIGKILL)`
    /// 杀整组，**不**依赖 `Child` 句柄（句柄已 move 进 wait 任务独占 await）。
    pid: Option<u32>,
}

/// `bash` 后台任务三件套的注册表。生产路径：`api/chat` 装配时 `Arc::new` 一份，
/// 通过 `AgentLoop::with_bash_task_registry` 注入；测试路径可注入 `tempfile::tempdir()`。
pub struct BashTaskRegistry {
    tasks: RwLock<HashMap<BashTaskId, Arc<BashTask>>>,
    persist_dir: PathBuf,
}

impl BashTaskRegistry {
    pub fn new(persist_dir: PathBuf) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            persist_dir,
        }
    }

    /// 起一个后台 bash：spawn + 起 stdout/stderr pump + 起 wait 任务回写状态。
    /// 立即返回 ticket，**不**等子进程结束。
    pub async fn spawn(
        &self,
        command: String,
        argv: Option<Vec<String>>,
        cwd: Option<PathBuf>,
    ) -> Result<BashTaskTicket, AppError> {
        std::fs::create_dir_all(&self.persist_dir).map_err(AppError::Io)?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let task_id = format!("{}-{}", now, simple_rand6());
        let log_path = self.persist_dir.join(format!("bash-{}.log", &task_id));

        let mut cmd = match argv.as_deref() {
            None => {
                #[cfg(unix)]
                let (shell, arg) = ("sh", "-c");
                #[cfg(windows)]
                let (shell, arg) = ("cmd", "/C");
                let mut c = Command::new(shell);
                c.arg(arg).arg(&command);
                c
            }
            Some(args) => {
                let mut c = Command::new(&command);
                c.args(args);
                c
            }
        };
        if let Some(c) = cwd.as_ref() {
            cmd.current_dir(c);
        }
        cmd.kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null());
        // 与 PR-E.2 同口径：新进程组 + stop 时 killpg 整组，避免 sh 派生孙子进程被遗弃。
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            .map_err(|e| AppError::Primitive(e.to_string()))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let log_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .map_err(AppError::Io)?;
        let log_writer = Arc::new(AsyncMutex::new(log_file));

        let task = Arc::new(BashTask {
            info: RwLock::new(BashTaskInfo {
                task_id: task_id.clone(),
                command: command.clone(),
                started_at_unix_ms: now,
                log_path: log_path.display().to_string(),
                status: BashTaskStatus::Running,
            }),
            pid,
        });
        self.tasks.write().insert(task_id.clone(), task.clone());

        // 两条 pump 任务：stdout / stderr 边读边追加日志。
        // stderr 行前缀 "STDERR: " 让 task_output 拉到的内容仍可肉眼区分两路。
        spawn_pump(stdout, log_writer.clone(), "");
        spawn_pump(stderr, log_writer.clone(), "STDERR: ");

        // wait 任务：独占 Child handle 等结束 → 翻 status。
        // 注意：stop 已把 status 置为 Stopped 时，**不**回退覆盖成 Finished。
        let task_for_wait = task.clone();
        tokio::spawn(async move {
            let exit_code = match child.wait().await {
                Ok(status) => {
                    #[cfg(unix)]
                    {
                        status
                            .code()
                            .or_else(|| status.signal().map(|s| 128 + s))
                            .unwrap_or(-1)
                    }
                    #[cfg(not(unix))]
                    {
                        status.code().unwrap_or(-1)
                    }
                }
                Err(_) => -1,
            };
            let mut info = task_for_wait.info.write();
            if !matches!(info.status, BashTaskStatus::Stopped) {
                info.status = BashTaskStatus::Finished { exit_code };
            }
        });

        Ok(BashTaskTicket {
            task_id,
            log_path: log_path.display().to_string(),
            started_at_unix_ms: now,
        })
    }

    /// 拉日志增量：`since=None` 从头读；返回 `[start_offset, next_offset)` 的字节窗口
    /// （UTF-8 lossy 解码）。`finished=true` 时 `exit_code` 一定有值。
    pub async fn read_output(
        &self,
        task_id: &str,
        since: Option<u64>,
    ) -> Result<BashTaskOutputChunk, AppError> {
        let task = self
            .tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::Primitive(format!("bash task not found: {}", task_id)))?;
        let info_snap = task.info.read().clone();
        let log_path = Path::new(&info_snap.log_path);
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(log_path)
            .await
            .map_err(AppError::Io)?;
        let start = since.unwrap_or(0);
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(AppError::Io)?;
        let mut buf = Vec::with_capacity(64 * 1024);
        file.read_to_end(&mut buf).await.map_err(AppError::Io)?;
        let next_offset = start + buf.len() as u64;
        let (finished, exit_code) = match info_snap.status {
            BashTaskStatus::Finished { exit_code } => (true, Some(exit_code)),
            BashTaskStatus::Stopped => (true, Some(-1)),
            BashTaskStatus::Running => (false, None),
        };
        Ok(BashTaskOutputChunk {
            task_id: task_id.to_string(),
            content: String::from_utf8_lossy(&buf).into_owned(),
            start_offset: start,
            next_offset,
            finished,
            exit_code,
        })
    }

    /// 主动停止：标记 status = Stopped → killpg(SIGKILL) 整组（Unix）；
    /// wait 任务后续 `child.wait()` 返回**不**回退覆盖（见 spawn 内 `if !matches!(...)`）。
    pub async fn stop(&self, task_id: &str) -> Result<(), AppError> {
        let task = self
            .tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::Primitive(format!("bash task not found: {}", task_id)))?;
        {
            let mut info = task.info.write();
            if matches!(info.status, BashTaskStatus::Running) {
                info.status = BashTaskStatus::Stopped;
            }
        }
        #[cfg(unix)]
        if let Some(pid) = task.pid {
            // SAFETY: POSIX 信号 API；pid 来自仍存活（或已 reaped）的子进程，
            // ESRCH 在已退场景下出现也无副作用。
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        // Windows 下不依赖 killpg；已设置 `kill_on_drop(true)`，且 wait 任务会
        // 在 Child drop 时由 tokio 兜底——此处不再重复 kill 以免 race。
        Ok(())
    }

    /// 全量枚举：按 started_at 升序，便于模型一眼看出"谁先起、谁还在跑"。
    pub fn list(&self) -> Vec<BashTaskInfo> {
        let mut v: Vec<BashTaskInfo> = self
            .tasks
            .read()
            .values()
            .map(|t| t.info.read().clone())
            .collect();
        v.sort_by_key(|i| i.started_at_unix_ms);
        v
    }
}

fn spawn_pump<R>(reader: Option<R>, writer: Arc<AsyncMutex<tokio::fs::File>>, prefix: &'static str)
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
{
    let Some(reader) = reader else {
        return;
    };
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        let mut buffered = BufReader::new(reader);
        loop {
            match buffered.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut f = writer.lock().await;
                    if !prefix.is_empty() {
                        let _ = f.write_all(prefix.as_bytes()).await;
                    }
                    let _ = f.write_all(&buf[..n]).await;
                    let _ = f.flush().await;
                }
                Err(_) => break,
            }
        }
    });
}

fn simple_rand6() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let chars = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut s = String::with_capacity(6);
    let mut x = nanos;
    for _ in 0..6 {
        s.push(chars[(x as usize) % chars.len()] as char);
        x = x.wrapping_mul(2_654_435_761).rotate_left(7) ^ (x >> 16);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_then_read_then_stop_then_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reg = BashTaskRegistry::new(dir.path().join("tool-results"));

        // 起一个会持续输出的后台任务（每 100ms 一行）。
        let ticket = reg
            .spawn(
                "i=0; while [ $i -lt 50 ]; do echo line-$i; i=$((i+1)); sleep 0.1; done"
                    .to_string(),
                None,
                None,
            )
            .await
            .expect("spawn");

        // 等几行写出来再拉。
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;

        let chunk1 = reg.read_output(&ticket.task_id, None).await.expect("read1");
        assert!(chunk1.start_offset == 0, "首读 since=None → start=0");
        assert!(
            chunk1.next_offset > 0,
            "应有字节读出，实际 = {}",
            chunk1.next_offset
        );
        assert!(!chunk1.finished, "Running 期间 finished=false");
        assert!(
            chunk1.content.contains("line-0"),
            "内容应含 line-0，实际 = {:?}",
            chunk1.content
        );

        // 续读：since=next_offset → 之间又有新行。
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let chunk2 = reg
            .read_output(&ticket.task_id, Some(chunk1.next_offset))
            .await
            .expect("read2");
        assert_eq!(chunk2.start_offset, chunk1.next_offset);

        // stop 后，list 中状态应为 Stopped。
        reg.stop(&ticket.task_id).await.expect("stop");
        // 给 wait 任务一点点时间让 child reap 完成（status 不会被覆盖）。
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let infos = reg.list();
        assert_eq!(infos.len(), 1);
        assert_eq!(
            infos[0].status,
            BashTaskStatus::Stopped,
            "stop 后 status 必须为 Stopped，不被 wait 任务回退覆盖"
        );

        // 再读一次：finished=true，exit_code=Some(-1)（Stopped）。
        let chunk_final = reg
            .read_output(&ticket.task_id, Some(0))
            .await
            .expect("read3");
        assert!(chunk_final.finished, "Stopped 后 finished=true");
        assert_eq!(chunk_final.exit_code, Some(-1));
    }

    #[tokio::test]
    async fn natural_finish_marks_finished_with_exit_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
        let ticket = reg
            .spawn("echo hi; exit 7".to_string(), None, None)
            .await
            .expect("spawn");
        // 等子进程自然结束。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let infos = reg.list();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].status, BashTaskStatus::Finished { exit_code: 7 });
        let chunk = reg.read_output(&ticket.task_id, None).await.expect("read");
        assert!(chunk.finished);
        assert_eq!(chunk.exit_code, Some(7));
        assert!(chunk.content.contains("hi"));
    }

    #[tokio::test]
    async fn read_output_unknown_task_id_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
        let err = reg
            .read_output("not-exist", None)
            .await
            .expect_err("应当 not found");
        assert!(format!("{}", err).contains("not found"));
    }
}
