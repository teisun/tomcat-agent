use std::io::{self, IsTerminal, Read};

use crate::infra::error::AppError;

pub(super) fn make_readline_editor() -> Result<rustyline::DefaultEditor, AppError> {
    rustyline::DefaultEditor::with_config(build_readline_config())
        .map_err(|e| AppError::Config(format!("初始化行编辑器失败: {}", e)))
}

pub(crate) fn build_readline_config() -> rustyline::Config {
    rustyline::Config::builder().bracketed_paste(true).build()
}

pub(crate) fn drain_pending_stdin_bytes() -> usize {
    drain_pending_stdin_bytes_impl().unwrap_or(0)
}

#[cfg(unix)]
fn drain_pending_stdin_bytes_impl() -> io::Result<usize> {
    use std::os::fd::AsRawFd;

    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Ok(0);
    }
    let fd = stdin.as_raw_fd();
    with_nonblocking_fd(fd, || {
        let mut lock = stdin.lock();
        drain_pending_bytes_from_reader(&mut lock)
    })
}

#[cfg(not(unix))]
fn drain_pending_stdin_bytes_impl() -> io::Result<usize> {
    Ok(0)
}

#[cfg(unix)]
fn with_nonblocking_fd<T, F>(fd: std::os::fd::RawFd, f: F) -> io::Result<T>
where
    F: FnOnce() -> io::Result<T>,
{
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = f();
    let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    result
}

pub(crate) fn drain_pending_bytes_from_reader<R: Read>(reader: &mut R) -> io::Result<usize> {
    let mut total = 0usize;
    let mut buf = [0u8; 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScriptedReader {
        steps: Vec<Result<Vec<u8>, io::ErrorKind>>,
    }

    impl Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.steps.is_empty() {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            match self.steps.remove(0) {
                Ok(bytes) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    Ok(n)
                }
                Err(kind) => Err(io::Error::from(kind)),
            }
        }
    }

    #[test]
    fn drain_pending_bytes_from_reader_drains_until_would_block() {
        let mut reader = ScriptedReader {
            steps: vec![
                Ok(b"first\n".to_vec()),
                Ok(b"second\n".to_vec()),
                Err(io::ErrorKind::WouldBlock),
            ],
        };
        let drained = drain_pending_bytes_from_reader(&mut reader).expect("drain");
        assert_eq!(drained, b"first\nsecond\n".len());
    }

    #[test]
    fn drain_pending_bytes_from_reader_noop_when_empty() {
        let mut reader = ScriptedReader {
            steps: vec![Err(io::ErrorKind::WouldBlock)],
        };
        let drained = drain_pending_bytes_from_reader(&mut reader).expect("drain");
        assert_eq!(drained, 0);
    }

    #[test]
    fn build_readline_config_enables_bracketed_paste() {
        let cfg = build_readline_config();
        assert!(cfg.enable_bracketed_paste());
    }
}
