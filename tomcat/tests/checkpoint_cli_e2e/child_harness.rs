use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct CapturedChildOutput {
    pub status: ExitStatus,
    pub stderr: String,
    pub stdout: String,
}

/// Owns the task group's PID file even if a test panics before task_stop.
#[cfg(unix)]
pub(super) struct BackgroundProcessGuard(std::path::PathBuf);

#[cfg(unix)]
impl BackgroundProcessGuard {
    pub(super) fn new(pid_path: std::path::PathBuf) -> Self {
        Self(pid_path)
    }
}

#[cfg(unix)]
impl Drop for BackgroundProcessGuard {
    fn drop(&mut self) {
        // The CLI (declared later) has been stopped. A shell already forked by it
        // may still be starting; allow its first PID-file write a bounded window.
        let deadline = Instant::now() + Duration::from_secs(2);
        let pid = loop {
            if let Some(pid) = std::fs::read_to_string(&self.0)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .filter(|pid| *pid > 1)
            {
                break pid;
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Product background bash uses process_group(0). Refuse a non-leader PID
        // rather than ever sending a signal to the test runner's own group.
        if unsafe { libc::getpgid(pid) } == pid {
            kill_process_group(pid as u32);
        }
    }
}

pub(super) struct CheckpointChild {
    child: Option<Child>,
    pid: u32,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
    stdin: Option<ChildStdin>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stdout_reader: Option<JoinHandle<()>>,
    stop_readers: Arc<AtomicBool>,
}

impl CheckpointChild {
    pub(super) fn spawn(command: &mut Command) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("chat child should start");
        let pid = child.id();
        let stdin = child.stdin.take().expect("stdin should be piped");
        let stdout = child.stdout.take().expect("stdout should be piped");
        let stderr = child.stderr.take().expect("stderr should be piped");
        let stdout_buffer = Arc::new(Mutex::new(Vec::new()));
        let stderr_buffer = Arc::new(Mutex::new(Vec::new()));
        #[cfg(unix)]
        {
            set_nonblocking(&stdout);
            set_nonblocking(&stderr);
        }
        let stop_readers = Arc::new(AtomicBool::new(false));
        let stdout_reader = Some(spawn_reader(
            stdout,
            Arc::clone(&stdout_buffer),
            Arc::clone(&stop_readers),
        ));
        let stderr_reader = Some(spawn_reader(
            stderr,
            Arc::clone(&stderr_buffer),
            Arc::clone(&stop_readers),
        ));
        Self {
            child: Some(child),
            pid,
            stderr: stderr_buffer,
            stderr_reader,
            stdin: Some(stdin),
            stdout: stdout_buffer,
            stdout_reader,
            stop_readers,
        }
    }

    pub(super) fn pid(&self) -> u32 {
        self.pid
    }

    pub(super) fn write_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("child stdin is closed");
        stdin.write_all(line.as_bytes()).expect("write child stdin");
        stdin.write_all(b"\n").expect("terminate child input line");
        stdin.flush().expect("flush child stdin");
    }

    pub(super) fn close_stdin(&mut self) {
        self.stdin.take();
    }

    pub(super) fn stderr_snapshot(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().expect("stderr buffer lock")).into_owned()
    }

    pub(super) fn stdout_snapshot(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().expect("stdout buffer lock")).into_owned()
    }

    pub(super) fn wait_for_stderr(&self, needle: &str, timeout: Duration) {
        self.wait_for_output("stderr", needle, timeout, || self.stderr_snapshot());
    }

    pub(super) fn wait_for_stdout(&self, needle: &str, timeout: Duration) {
        self.wait_for_output("stdout", needle, timeout, || self.stdout_snapshot());
    }

    fn wait_for_output(
        &self,
        stream_name: &str,
        needle: &str,
        timeout: Duration,
        snapshot: impl Fn() -> String,
    ) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if snapshot().contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "child pid={} {stream_name} did not contain {needle:?} within {timeout:?}; diagnostics={}",
            self.pid,
            self.diagnostics(),
        );
    }

    pub(super) fn finish(mut self, timeout: Duration) -> CapturedChildOutput {
        self.close_stdin();
        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            let child = self.child.as_mut().expect("child process missing");
            if let Some(status) = child.try_wait().expect("query child status") {
                break status;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                terminate_process_tree(child, self.pid);
                break wait_for_exit(child, Duration::from_secs(2))
                    .expect("reap timed-out child within kill budget");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // The direct child may exit while a descendant still owns its pipes.
        kill_process_group(self.pid);
        self.child.take();
        let drained = self.join_readers();
        let output = CapturedChildOutput {
            status,
            stderr: self.stderr_snapshot(),
            stdout: self.stdout_snapshot(),
        };
        if timed_out || !drained {
            panic!(
                "checkpoint child timeout/incomplete pipes after {timeout:?}; drained={drained}; pid={}; status={}; stdout={:?}; stderr={:?}",
                self.pid,
                output.status,
                output.stdout,
                output.stderr,
            );
        }
        output
    }

    fn diagnostics(&self) -> String {
        format!(
            "pid={} stdout={:?} stderr={:?}",
            self.pid,
            self.stdout_snapshot(),
            self.stderr_snapshot(),
        )
    }

    fn join_readers(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        let readers = [&mut self.stdout_reader, &mut self.stderr_reader];
        while readers
            .iter()
            .any(|reader| reader.as_ref().is_some_and(|r| !r.is_finished()))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let drained = readers
            .iter()
            .all(|reader| reader.as_ref().is_none_or(|r| r.is_finished()));
        self.stop_readers.store(true, Ordering::SeqCst);
        let cancel_deadline = Instant::now() + Duration::from_millis(200);
        for slot in readers {
            if let Some(reader) = slot.take() {
                while !reader.is_finished() && Instant::now() < cancel_deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if reader.is_finished() {
                    let _ = reader.join();
                }
                // On non-Unix a blocking reader may not be interruptible. Never
                // allow joining it to hang the test; finish reports undrained IO.
            }
        }
        drained
    }
}

impl Drop for CheckpointChild {
    fn drop(&mut self) {
        self.stdin.take();
        kill_process_group(self.pid);
        if let Some(mut child) = self.child.take() {
            terminate_process_tree(&mut child, self.pid);
            if wait_for_exit(&mut child, Duration::from_secs(2)).is_none() {
                eprintln!("checkpoint cleanup failed to reap pid={}", self.pid);
            }
        }
        if !self.join_readers() {
            eprintln!("checkpoint cleanup incomplete pipes pid={}", self.pid);
        }
    }
}

fn terminate_process_tree(child: &mut Child, pid: u32) {
    kill_process_group(pid);
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
}

fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn set_nonblocking(pipe: &impl std::os::fd::AsRawFd) {
    unsafe {
        let fd = pipe.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        assert!(
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0,
            "configure cancellable child pipe: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn exited_parent_does_not_leave_descendants_holding_pipes() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & printf ready; exit 0"]);
        let child = CheckpointChild::spawn(&mut command);
        child.wait_for_stdout("ready", Duration::from_secs(2));
        let started = Instant::now();
        assert!(child.finish(Duration::from_secs(2)).status.success());
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn drains_large_output_without_deadlock_and_bounds_retained_tail() {
        let mut command = Command::new("sh");
        command.args(["-c", "awk 'BEGIN {for(i=0;i<50000;i++) print \"012345678901234567890123456789\"; print \"TAIL_OK\"}'"]);
        let output = CheckpointChild::spawn(&mut command).finish(Duration::from_secs(5));
        assert!(output.status.success());
        assert!(output.stdout.ends_with("TAIL_OK\n"));
        assert!(output.stdout.len() <= 1024 * 1024);
    }

    #[test]
    fn panic_cleanup_reaps_the_owned_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf ready; sleep 30"]);
        let child = CheckpointChild::spawn(&mut command);
        let pid = child.pid();
        child.wait_for_stdout("ready", Duration::from_secs(2));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _owned = child;
            panic!("fixture initialization failed");
        }))
        .is_err());
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }

    #[test]
    fn panic_and_timeout_cleanup_own_a_separate_background_group() {
        use std::os::unix::process::CommandExt;
        for timeout in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let pid_path = dir.path().join("task.pid");
            let guard = BackgroundProcessGuard::new(pid_path.clone());
            let mut background = Command::new("sh")
                .args(["-c", "printf '%s' $$ > \"$1\"; sleep 30", "fixture"])
                .arg(&pid_path)
                .process_group(0)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !pid_path.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _owned_task = guard;
                let mut command = Command::new("sh");
                command.args(["-c", "printf ready; sleep 30"]);
                let child = CheckpointChild::spawn(&mut command);
                child.wait_for_stdout("ready", Duration::from_secs(2));
                if timeout {
                    child.finish(Duration::from_millis(40));
                } else {
                    panic!("failure before task_stop");
                }
            }));
            let status = wait_for_exit(&mut background, Duration::from_secs(2));
            if status.is_none() {
                let pid = background.id();
                terminate_process_tree(&mut background, pid);
                let _ = wait_for_exit(&mut background, Duration::from_secs(2));
            }
            assert!(outcome.is_err());
            assert!(
                status.is_some_and(|status| !status.success()),
                "separate background must be killed before fixture removal"
            );
        }
    }

    #[test]
    fn timeout_kills_reaps_and_drains_the_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'ready'; printf 'diagnostic' >&2; sleep 30"]);
        let child = CheckpointChild::spawn(&mut command);
        let pid = child.pid();
        child.wait_for_stdout("ready", Duration::from_secs(2));
        child.wait_for_stderr("diagnostic", Duration::from_secs(2));

        let timed_out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            child.finish(Duration::from_millis(40));
        }));
        assert!(timed_out.is_err(), "timeout must report structured failure");
        let alive = unsafe { libc::kill(pid as i32, 0) };
        assert_eq!(alive, -1, "timed-out child must already be reaped");
    }
}

fn spawn_reader<R>(
    mut reader: R,
    output: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while !stop.load(Ordering::SeqCst) {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(size) => {
                    let mut buffer = output.lock().expect("child output buffer lock");
                    buffer.extend_from_slice(&chunk[..size]);
                    let excess = buffer.len().saturating_sub(1024 * 1024);
                    if excess > 0 {
                        buffer.drain(..excess);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}
