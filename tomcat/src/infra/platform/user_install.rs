//! 用户级命令安装：把可执行文件装进用户命令目录，并把该目录加入用户 PATH。
//!
//! - Unix：命令入口是指向 exe 的软链；PATH 写进 shell 启动脚本（按 `$SHELL` 选择）。
//! - Windows：命令入口是 exe 的副本；PATH 写进 `HKCU\Environment\Path`。不用 `setx`：它会把
//!   PATH 截断到 1024 字符，并把 `%VAR%` 展开后写死。

use std::io;
use std::path::{Path, PathBuf};

use super::canonicalize;

/// 自动配置 PATH 做不到时，用户需要手动完成的一步。这里只给数据，文案由调用方渲染。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualPathStep {
    /// 在 shell 里执行这一行（Unix）。
    RunInShell(String),
    /// 把这个目录加入用户 PATH 环境变量（Windows 系统设置）。
    AddUserPathEntry(PathBuf),
}

/// 用户命令目录 `~/.local/bin`（XDG 规范；Windows 沿用同一目录）。
pub fn user_bin_dir(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

/// 让用户手动把 `dir` 加入 PATH 的那一步（例如找不到 HOME、无法自动配置时）。
pub fn manual_path_step(dir: &Path) -> ManualPathStep {
    if cfg!(windows) {
        ManualPathStep::AddUserPathEntry(dir.to_path_buf())
    } else {
        ManualPathStep::RunInShell(format!("export PATH=\"{}:$PATH\"", dir.display()))
    }
}

/// 在 `dir` 里安装名为 `name` 的命令入口（Windows 自动加 `.exe`），指向 `exe`。
/// 返回新装或更新的入口；已是最新、或入口位置被不该碰的文件占用时返回 `None`。
///
/// - 已有软链：指向同一个 exe 就跳过，指向别处就替换。
/// - 已有普通文件：Unix 不碰（可能是用户自己装的）；Windows 视为上次复制的副本，内容不同才替换。
///
/// # Errors
/// * [`io::Error`] - `exe` 无法 canonicalize，或目录/入口无法创建、替换。
pub fn install_user_command(exe: &Path, dir: &Path, name: &str) -> io::Result<Option<PathBuf>> {
    let exe = canonicalize(exe)?;
    std::fs::create_dir_all(dir)?;
    let entry = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    remove_parked_copies(&entry);

    match std::fs::symlink_metadata(&entry) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if canonicalize(&entry).ok().as_deref() == Some(exe.as_path()) {
                return Ok(None);
            }
            std::fs::remove_file(&entry)?;
        }
        Ok(_) => return refresh_regular_entry(&exe, &entry),
        Err(_) => {}
    }
    create_entry(&exe, &entry)?;
    Ok(Some(entry))
}

#[cfg(unix)]
fn create_entry(exe: &Path, entry: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(exe, entry)
}

#[cfg(windows)]
fn create_entry(exe: &Path, entry: &Path) -> io::Result<()> {
    std::fs::copy(exe, entry).map(|_| ())
}

#[cfg(unix)]
fn refresh_regular_entry(_exe: &Path, _entry: &Path) -> io::Result<Option<PathBuf>> {
    Ok(None)
}

/// 正在运行的 exe 不能覆盖或删除，但可以改名：先把旧副本改名成 `*.old` 再复制；
/// 删不掉的 `*.old` 留给下次安装时清理。
#[cfg(windows)]
fn refresh_regular_entry(exe: &Path, entry: &Path) -> io::Result<Option<PathBuf>> {
    if std::fs::read(entry)? == std::fs::read(exe)? {
        return Ok(None);
    }
    let parked = entry.with_file_name(format!(
        "{}.{}{PARKED_COPY_SUFFIX}",
        file_name(entry),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::rename(entry, &parked)?;
    if let Err(error) = std::fs::copy(exe, entry) {
        let _ = std::fs::rename(&parked, entry);
        return Err(error);
    }
    let _ = std::fs::remove_file(&parked);
    Ok(Some(entry.to_path_buf()))
}

#[cfg(windows)]
const PARKED_COPY_SUFFIX: &str = ".old";

#[cfg(windows)]
fn file_name(entry: &Path) -> String {
    entry
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(unix)]
fn remove_parked_copies(_entry: &Path) {}

/// 清理上次替换时因旧副本仍在运行而没删掉的 `<入口名>.*.old`；仍在运行的会删除失败，跳过即可。
#[cfg(windows)]
fn remove_parked_copies(entry: &Path) {
    let Some(dir) = entry.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{}.", file_name(entry));
    for item in entries.flatten() {
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(PARKED_COPY_SUFFIX) {
            let _ = std::fs::remove_file(item.path());
        }
    }
}

#[cfg(unix)]
const LOCAL_BIN_EXPORT_LINE: &str = "export PATH=\"$HOME/.local/bin:$PATH\"";
#[cfg(unix)]
const TOMCAT_INIT_COMMENT: &str = "# Added by tomcat init";

/// 把 [`user_bin_dir`] 加入用户 PATH，之后新开的终端生效；已加入时什么也不改。
///
/// Unix 按 `$SHELL` 写 `.zprofile`/`.zshrc`、`.bashrc` 或 `.profile`，并清理旧版本写入的
/// `/target/` export 行。
///
/// # Errors
/// 自动配置失败时返回用户需要手动完成的 [`ManualPathStep`]。
#[cfg(unix)]
pub fn add_user_bin_dir_to_path(home: &Path) -> Result<(), ManualPathStep> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let mut ok = true;
    for profile in path_export_targets(&shell, home) {
        ok &= prune_stale_tomcat_exports(&profile);
        ok &= append_export_once(&profile, LOCAL_BIN_EXPORT_LINE);
    }
    if shell.contains("bash") {
        // macOS 的 login bash 会优先读 .bash_profile（存在时不再继续读 .profile），而
        // 交互式 bash 通常读 .bashrc。为避免截断用户写在 .profile 里的通用环境（如 Rust 的
        // ~/.cargo/env），PATH 仍写进 .bashrc，再确保 .bash_profile 同时 source
        // .profile 与 .bashrc。
        ok &= ensure_bash_profile_sources_profile_and_bashrc(home);
    }
    if ok {
        Ok(())
    } else {
        Err(ManualPathStep::RunInShell(
            LOCAL_BIN_EXPORT_LINE.to_string(),
        ))
    }
}

/// 把 [`user_bin_dir`] 加入用户 PATH，之后新开的终端生效；已加入时什么也不改。
///
/// 只有 HOME 就是系统登记的用户目录时才改注册表：测试/沙箱用 HOME 做隔离时绝不碰真实 PATH。
///
/// # Errors
/// 自动配置失败时返回用户需要手动完成的 [`ManualPathStep`]。
#[cfg(windows)]
pub fn add_user_bin_dir_to_path(home: &Path) -> Result<(), ManualPathStep> {
    let dir = user_bin_dir(home);
    let added = is_system_user_home(home)
        && match add_to_user_path(USER_ENVIRONMENT_KEY, &dir.display().to_string()) {
            Ok(changed) => {
                if changed {
                    broadcast_environment_change();
                }
                true
            }
            Err(_) => false,
        };
    if added {
        Ok(())
    } else {
        Err(ManualPathStep::AddUserPathEntry(dir))
    }
}

#[cfg(unix)]
fn path_export_targets(shell: &str, home: &Path) -> Vec<PathBuf> {
    if shell.contains("zsh") {
        return vec![home.join(".zprofile"), home.join(".zshrc")];
    }
    if shell.contains("bash") {
        return vec![home.join(".bashrc")];
    }
    vec![home.join(".profile")]
}

/// 幂等地把一行 export 追加到指定 shell 启动脚本；已存在同序 export 则跳过。
#[cfg(unix)]
fn append_export_once(profile: &Path, export_line: &str) -> bool {
    use std::io::Write;

    if let Ok(content) = std::fs::read_to_string(profile) {
        if content.contains(export_line) {
            return true;
        }
    }
    let mut f = match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(profile)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    writeln!(f, "\n{}\n{}", TOMCAT_INIT_COMMENT, export_line).is_ok()
}

#[cfg(unix)]
fn is_stale_tomcat_target_export(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("export PATH=") && trimmed.contains("/target/")
}

#[cfg(unix)]
fn prune_stale_lines(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut result = Vec::with_capacity(lines.len());
    let mut changed = false;
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.trim() == TOMCAT_INIT_COMMENT
            && lines
                .get(index + 1)
                .is_some_and(|next| is_stale_tomcat_target_export(next))
        {
            changed = true;
            index += 2;
            continue;
        }
        if is_stale_tomcat_target_export(line) {
            changed = true;
            index += 1;
            continue;
        }
        result.push(line);
        index += 1;
    }

    if !changed {
        return content.to_string();
    }

    let mut pruned = result.join("\n");
    if content.ends_with('\n') {
        pruned.push('\n');
    }
    pruned
}

#[cfg(unix)]
fn prune_stale_tomcat_exports(profile: &Path) -> bool {
    let content = match std::fs::read_to_string(profile) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let pruned = prune_stale_lines(&content);
    if pruned == content {
        return true;
    }
    super::write_file_atomic(profile, pruned.as_bytes()).is_ok()
}

/// 确保 `~/.bash_profile` 会同时 source `~/.profile` 与 `~/.bashrc`，使 macOS 上的
/// login bash 既保留用户已有的通用环境变量，也能加载写入 `.bashrc` 的 PATH。
#[cfg(unix)]
fn ensure_bash_profile_sources_profile_and_bashrc(home: &Path) -> bool {
    use std::io::Write;

    let bash_profile = home.join(".bash_profile");
    let profile_snippet = "[ -r \"$HOME/.profile\" ] && . \"$HOME/.profile\"";
    let bashrc_snippet = "[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"";
    let content = std::fs::read_to_string(&bash_profile).unwrap_or_default();
    let needs_profile = !content.contains(profile_snippet);
    let needs_bashrc = !content.contains(bashrc_snippet);
    if !needs_profile && !needs_bashrc {
        return true;
    }
    let mut f = match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&bash_profile)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    if writeln!(f, "\n{TOMCAT_INIT_COMMENT}").is_err() {
        return false;
    }
    if needs_profile && writeln!(f, "{}", profile_snippet).is_err() {
        return false;
    }
    if needs_bashrc && writeln!(f, "{}", bashrc_snippet).is_err() {
        return false;
    }
    true
}

/// 生产代码使用的用户环境变量子键；测试传临时子键。
#[cfg(windows)]
const USER_ENVIRONMENT_KEY: &str = "Environment";

#[cfg(windows)]
const ERROR_FILE_NOT_FOUND_HRESULT: i32 = 0x8007_0002_u32 as i32;

/// `dirs::home_dir` 在 Windows 读系统登记的用户目录，不看 HOME/USERPROFILE。
#[cfg(windows)]
fn is_system_user_home(home: &Path) -> bool {
    let canonical = |path: &Path| canonicalize(path).ok();
    dirs::home_dir()
        .and_then(|system| canonical(&system))
        .is_some_and(|system| canonical(home).as_ref() == Some(&system))
}

/// 把 `entry` 追加到 `current` 末尾；已包含（忽略大小写、末尾 `\`，`%VAR%` 按当前环境
/// 展开后比较）时返回 `None`。保留其它条目的原有顺序，去掉空条目。
#[cfg(windows)]
fn merge_user_path(current: &str, entry: &str) -> Option<String> {
    let wanted = comparable(entry);
    let mut entries: Vec<&str> = current
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect();
    if entries
        .iter()
        .any(|item| comparable(&expand_env(item)) == wanted)
    {
        return None;
    }
    entries.push(entry);
    Some(entries.join(";"))
}

#[cfg(windows)]
fn comparable(entry: &str) -> String {
    entry.trim().trim_end_matches(['\\', '/']).to_lowercase()
}

#[cfg(windows)]
fn expand_env(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else {
            break;
        };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(expanded) if !name.is_empty() => out.push_str(&expanded),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

/// 把 `entry` 加入 `HKCU\<subkey>\Path`，保留原值类型：写成 REG_SZ 会让 PATH 里其它
/// `%VAR%` 条目失效。返回 `true` 表示写入了，`false` 表示早已包含。
#[cfg(windows)]
fn add_to_user_path(subkey: &str, entry: &str) -> windows_registry::Result<bool> {
    use windows_registry::{Type, CURRENT_USER};

    let key = CURRENT_USER.create(subkey)?;
    let (current, kind) = match key.get_value("Path") {
        Ok(value) => {
            let kind = value.ty();
            (String::try_from(value)?, kind)
        }
        Err(error) if error.code().0 == ERROR_FILE_NOT_FOUND_HRESULT => {
            (String::new(), Type::ExpandString)
        }
        Err(error) => return Err(error),
    };
    let Some(merged) = merge_user_path(&current, entry) else {
        return Ok(false);
    };
    match kind {
        Type::String => key.set_string("Path", &merged)?,
        _ => key.set_expand_string("Path", &merged)?,
    }
    Ok(true)
}

/// 通知资源管理器等程序重新读取环境变量，之后新开的终端才能看到新 PATH；失败不影响结果。
#[cfg(windows)]
fn broadcast_environment_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };
    let area: Vec<u16> = "Environment\0".encode_utf16().collect();
    let mut result = 0;
    // SAFETY: `area` is a NUL-terminated UTF-16 string that outlives the synchronous call.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            area.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        );
    }
}

#[cfg(test)]
#[path = "tests/user_install_test.rs"]
mod tests;
