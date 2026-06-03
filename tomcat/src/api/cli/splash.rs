//! `tomcat chat` 启动像素风吉祥物 Splash（「Tommy」像素猫）。
//!
//! 设计要点（见 plan「Tomcat 像素风 Splash 吉祥物设计方案」）：
//! - 仅绘制吉祥物像素画，**不**输出任何文本 banner —— banner 仍由 `chat_loop` 负责，文案一字不改。
//! - **TTY 守卫**：仅当 stdout 为真实终端时才绘制并使用光标转义；非 TTY（管道 / 重定向 / CI /
//!   `cli_tests.rs` 抓 stdout）一律降级为「什么都不打印」，保证既有断言与脚本零回归。
//! - `TOMCAT_SPLASH=0` 强制关闭；`NO_COLOR` 去色但保留字符帧。
//! - 4 帧 idle 动画（400ms/帧，循环 1 次后定格首帧），不进入持续刷新，规避读屏器骚扰。
//!
//! 帧素材为编译期 `include_str!` 嵌入（参考 `core::prompts` 的做法），运行期不读盘。
//!
//! 颜色实现取舍：帧文件保持纯文本，渲染时对**整块**吉祥物套用橙色（256-color 208），
//! 不做逐字符的蓝耳尖 / dim 描边——以换取帧素材简单、渲染稳健。

use std::io::{IsTerminal, Write};
use std::time::Duration;

use crate::infra::config::SplashConfig;

/// idle 动画总帧数。
const FRAME_COUNT: usize = 4;
/// 每帧停留时长。FRAME_COUNT 帧 + 1 次回到首帧 ≈ 1.6s。
const FRAME_INTERVAL: Duration = Duration::from_millis(400);
/// 居中左缩进的安全上限：避免在窄终端上把吉祥物推出可视区导致折行。
const MAX_SAFE_INDENT: usize = 12;
/// 整块吉祥物的橙色前景（ANSI 256-color）。
const ORANGE: &str = "\x1b[38;5;208m";
const RESET: &str = "\x1b[0m";

const FRAMES: [&str; FRAME_COUNT] = [
    include_str!("../../../assets/splash/tommy_braille/frame_0.txt"),
    include_str!("../../../assets/splash/tommy_braille/frame_1.txt"),
    include_str!("../../../assets/splash/tommy_braille/frame_2.txt"),
    include_str!("../../../assets/splash/tommy_braille/frame_3.txt"),
];

fn frames() -> &'static [&'static str; FRAME_COUNT] {
    &FRAMES
}

/// idle 动画帧数（编译期 `include_str!` 嵌入）。
pub(crate) fn splash_frame_count() -> usize {
    frames().len()
}

/// 是否已加载全部非空帧素材。
pub(crate) fn splash_frames_loaded() -> bool {
    frames().iter().all(|f| !f.trim().is_empty())
}

/// 拆分单帧为行（去掉尾随空行）。
fn frame_lines(frame: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = frame.split('\n').collect();
    while matches!(lines.last(), Some(l) if l.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// 一组帧统一的绘制高度（取各帧最大行数，保证动画期间垂直不抖动）。
fn block_height(frames: &[&str]) -> usize {
    frames
        .iter()
        .map(|f| frame_lines(f).len())
        .max()
        .unwrap_or(0)
}

/// 一组帧统一的绘制宽度（取各帧各行最大字符数，保证动画期间水平不抖动）。
fn block_width(frames: &[&str]) -> usize {
    frames
        .iter()
        .flat_map(|f| frame_lines(f))
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
}

/// 依据参考宽度计算居中左缩进；按 [`MAX_SAFE_INDENT`] 夹断以适配窄终端。
fn indent_for(width: usize, max_width: usize) -> usize {
    (max_width.saturating_sub(width) / 2).min(MAX_SAFE_INDENT)
}

/// 渲染单帧为「height 行、左缩进、可选橙色」的字符串（不含光标转义）。
/// 短于 height 的帧用空行补齐，使动画各帧高度一致。
fn render_frame(frame: &str, indent: usize, color: bool, height: usize) -> String {
    let pad = " ".repeat(indent);
    let lines = frame_lines(frame);
    let mut out = String::new();
    for i in 0..height {
        let content = lines.get(i).copied().unwrap_or("");
        if content.is_empty() {
            // 保留空行，无需缩进 / 颜色。
        } else if color {
            out.push_str(&pad);
            out.push_str(ORANGE);
            out.push_str(content);
            out.push_str(RESET);
        } else {
            out.push_str(&pad);
            out.push_str(content);
        }
        if i + 1 < height {
            out.push('\n');
        }
    }
    out
}

/// 渲染静态首帧（F0）为字符串。供「动画关闭」分支复用。
fn render_static(color: bool, max_width: usize) -> String {
    let fs = frames();
    let width = block_width(fs);
    let indent = indent_for(width, max_width);
    let height = block_height(fs);
    render_frame(fs[0], indent, color, height)
}

/// 判断是否应当绘制 Splash：综合环境变量、配置开关与 TTY 守卫。
fn should_render(cfg: &SplashConfig) -> bool {
    if matches!(std::env::var("TOMCAT_SPLASH").as_deref(), Ok("0")) {
        return false;
    }
    if !cfg.enabled {
        return false;
    }
    // TTY 守卫：非交互终端一律不绘制，避免污染管道输出 / 测试断言。
    std::io::stdout().is_terminal()
}

/// 在 `tomcat chat` 启动时绘制吉祥物。
///
/// 仅负责吉祥物像素画；后续文本 banner 由 `chat_loop` 照常打印。
/// 在不满足绘制条件时静默返回（无任何输出）。
pub fn render_mascot(cfg: &SplashConfig) {
    if !should_render(cfg) {
        return;
    }
    let color = std::env::var_os("NO_COLOR").is_none();
    let fs = frames();
    let width = block_width(fs);
    let indent = indent_for(width, cfg.max_width);
    let height = block_height(fs);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if cfg.animations {
        // 首帧
        let _ = write!(out, "{}", render_frame(fs[0], indent, color, height));
        let _ = out.flush();
        // 依次播放 F1 → F2 → F3 → 回到 F0 定格；每次回到块顶用 `\x1b[K` 清行重绘。
        let sequence = [fs[1], fs[2], fs[3], fs[0]];
        for frame in sequence {
            std::thread::sleep(FRAME_INTERVAL);
            // 光标回到块顶（上移 height-1 行 + 行首），逐行清屏重绘。
            let _ = write!(out, "\r\x1b[{}A", height.saturating_sub(1));
            for i in 0..height {
                let _ = write!(out, "\x1b[K");
                let body = render_single_line(frame, indent, color, i);
                let _ = write!(out, "{}", body);
                if i + 1 < height {
                    let _ = writeln!(out);
                }
            }
            let _ = out.flush();
        }
        let _ = writeln!(out);
    } else {
        let _ = writeln!(out, "{}", render_static(color, cfg.max_width));
    }
    // 与后续 banner 之间留一个空行。
    let _ = writeln!(out);
    let _ = out.flush();
}

/// 渲染单帧的第 `idx` 行（含缩进 / 颜色），供动画逐行重绘使用。
fn render_single_line(frame: &str, indent: usize, color: bool, idx: usize) -> String {
    let lines = frame_lines(frame);
    let content = lines.get(idx).copied().unwrap_or("");
    if content.is_empty() {
        return String::new();
    }
    let pad = " ".repeat(indent);
    if color {
        format!("{}{}{}{}", pad, ORANGE, content, RESET)
    } else {
        format!("{}{}", pad, content)
    }
}

#[cfg(test)]
#[path = "tests/splash_test.rs"]
mod tests;
