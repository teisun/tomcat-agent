//! `tomcat init` 在 Windows 上把命令目录写进用户级 PATH（`HKCU\Environment\Path`）。
//!
//! 不用 `setx`：它会把 PATH 截断到 1024 字符，并把 `%VAR%` 展开后写死。

use windows_registry::{Type, CURRENT_USER};

/// 生产代码使用的用户环境变量子键；测试传临时子键。
pub(crate) const USER_ENVIRONMENT_KEY: &str = "Environment";

const ERROR_FILE_NOT_FOUND_HRESULT: i32 = 0x8007_0002_u32 as i32;

/// 把 `entry` 追加到 `current` 末尾；已包含（忽略大小写、末尾 `\`，`%VAR%` 按当前环境
/// 展开后比较）时返回 `None`。保留其它条目的原有顺序，去掉空条目。
pub(crate) fn merge_user_path(current: &str, entry: &str) -> Option<String> {
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

fn comparable(entry: &str) -> String {
    entry.trim().trim_end_matches(['\\', '/']).to_lowercase()
}

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
pub(crate) fn add_to_user_path(subkey: &str, entry: &str) -> windows_registry::Result<bool> {
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
pub(crate) fn broadcast_environment_change() {
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
