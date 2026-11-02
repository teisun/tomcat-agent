//! 唯一的平台差异层：所有和操作系统打交道的代码都在这里，对上层只暴露统一接口。
//!
//! 业务代码不写 `cfg(unix/windows/target_os)`，也不直接用 `std::os::*`、`libc`、`windows-sys`
//! 等系统 crate（`tests/boundary_test.rs` 守护）。判断依据是"操作系统的事实"时，标准库已提供的
//! （`Path::is_absolute`、`std::env::consts::EXE_SUFFIX`、`std::path::is_separator`）直接用标准库。

mod console;
mod environment;
mod executable;
mod fs;
pub(crate) mod local_socket;
mod paths;
#[cfg(windows)]
mod private_acl;
mod process;
#[cfg(windows)]
mod process_job;
mod shell;
pub(crate) mod stdin;
mod system;
mod user_install;

pub(crate) use console::prepare_console;
pub use console::ASYNC_PRINT_DURING_INPUT;
pub use environment::EnvironmentSnapshot;
pub use executable::{find_executable, resolve_program};
pub(crate) use fs::is_lock_contended;
pub use fs::{
    mode_bits, open_no_follow, read_file_utf8, same_file, set_create_mode,
    set_private_file_permissions, write_file_atomic, write_file_atomic_with,
    write_private_file_atomic, PermissionSnapshot, NULL_DEVICE,
};
pub use paths::{
    canonicalize, format_home_path, home_dir, normalize_path, path_starts_with, to_slash,
    LEGACY_MAX_PATH, PATH_COMPARE_IGNORES_CASE,
};
pub use process::{
    exit_code, kill_tree_on_drop, spawn_detached, spawn_service_process, ProcessTree,
    CHILD_STARTUP_ENV_VARS,
};
pub use shell::{posix_shell, posix_shell_for, shell_path_to_native, POSIX_SHELL_NEEDS_INSTALL};
pub use system::{
    open_url, system_package_manager, PackageManager, DEFAULT_EDITOR, FOLLOW_FILE_COMMAND,
    PATH_CHANGES_NEED_RESTART,
};
pub use user_install::{
    add_user_bin_dir_to_path, install_user_command, manual_path_step, user_bin_dir, ManualPathStep,
};

#[cfg(test)]
mod tests;
