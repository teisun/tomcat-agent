//! Same-user local IPC. Unix socket files are removed only by the startup-lock owner,
//! never by a late listener destructor that could unlink a replacement instance.
use std::io;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncWrite};

pub trait LocalIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> LocalIo for T {}
pub type LocalStream = Box<dyn LocalIo>;

pub fn endpoint(service_dir: &Path) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        let path = service_dir.join("serve.sock");
        std::os::unix::net::SocketAddr::from_pathname(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("invalid local service socket path {}: {e}", path.display()),
            )
        })?;
        Ok(path)
    }
    #[cfg(windows)]
    {
        use sha2::{Digest, Sha256};
        // Windows paths compare case-insensitively. The agent ID is already in this path.
        let key = service_dir
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase();
        Ok(PathBuf::from(format!(
            r"\\.\pipe\tomcat-{:x}",
            Sha256::digest(key.as_bytes())
        )))
    }
}

pub fn prepare_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "service directory must be a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: read-only query of this process's effective UID.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "service directory belongs to another user",
            ));
        }
        for parent in path.ancestors().skip(1) {
            let metadata = std::fs::metadata(parent)?;
            if (metadata.uid() != uid && metadata.uid() != 0)
                || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("unsafe service directory ancestor: {}", parent.display()),
                ));
            }
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    super::private_acl::restrict_to_current_user(path)?;
    Ok(())
}

/// Call only while holding the service startup lock, after a failed connection probe.
pub fn remove_stale_endpoint(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        // SAFETY: read-only UID query. Never remove a regular file or arbitrary link.
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "refusing to replace a non-socket or foreign local endpoint",
            ));
        }
        std::fs::remove_file(path)?;
    }
    #[cfg(windows)]
    let _ = path; // The kernel removes a named pipe when its final handle closes.
    Ok(())
}

pub async fn connect(path: &Path) -> io::Result<LocalStream> {
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(path).await?;
        verify_unix_peer(&stream)?;
        Ok(Box::new(stream))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
        use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
        let stream = loop {
            match tokio::net::windows::named_pipe::ClientOptions::new().open(path) {
                Ok(stream) => break stream,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await
                }
                Err(e) => return Err(e),
            }
        };
        let mut pid = 0;
        // SAFETY: stream owns a live pipe; PID is a writable output variable.
        if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle(), &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // Authenticate the server BEFORE sending environment snapshots or other credentials.
        super::private_acl::verify_process_user(pid)?;
        Ok(Box::new(stream))
    }
}

pub struct LocalListener {
    #[cfg(unix)]
    listener: tokio::net::UnixListener,
    #[cfg(windows)]
    listener: tokio::net::windows::named_pipe::NamedPipeServer,
    #[cfg(windows)]
    path: PathBuf,
}

impl LocalListener {
    /// Does not unlink anything. Even an uncoordinated child cannot replace a live listener.
    pub fn bind(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let listener = tokio::net::UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self { listener })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                listener: private_pipe(path, true)?,
                path: path.to_owned(),
            })
        }
    }

    pub async fn accept(&mut self) -> io::Result<LocalStream> {
        #[cfg(unix)]
        {
            let (stream, _) = self.listener.accept().await?;
            verify_unix_peer(&stream)?;
            Ok(Box::new(stream))
        }
        #[cfg(windows)]
        {
            self.listener.connect().await?;
            let next = private_pipe(&self.path, false)?;
            Ok(Box::new(std::mem::replace(&mut self.listener, next)))
        }
    }
}

#[cfg(unix)]
fn verify_unix_peer(stream: &tokio::net::UnixStream) -> io::Result<()> {
    // SAFETY: read-only effective UID query; peer_cred is supplied by the kernel.
    if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local service peer belongs to another user",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn private_pipe(
    path: &Path,
    first: bool,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use windows_sys::Win32::Security::{
        InitializeSecurityDescriptor, SetSecurityDescriptorDacl, SECURITY_ATTRIBUTES,
        SECURITY_DESCRIPTOR,
    };
    let mut acl = super::private_acl::current_user_acl()?;
    // SAFETY: zero initialization is valid for this plain Windows descriptor struct.
    let mut descriptor: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
    let descriptor_ptr = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
    // SAFETY: descriptor and the aligned, initialized ACL remain live through pipe creation.
    if unsafe { InitializeSecurityDescriptor(descriptor_ptr, 1) } == 0
        || unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, acl.as_mut_ptr().cast(), 0) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor_ptr,
        bInheritHandle: 0,
    };
    // SAFETY: attributes points to a valid descriptor with one ACE, for the current user only.
    unsafe {
        tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                path,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
    }
}

#[cfg(test)]
#[path = "tests/local_socket_test.rs"]
mod tests;

pub async fn termination_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = term.recv() => Ok(()) }
    }
    #[cfg(windows)]
    {
        tokio::signal::ctrl_c().await
    }
}
