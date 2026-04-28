//! # SessionGrants & DraggedPaths
//!
//! 进程内共享的临时授权缓存：
//!
//! - **`SessionGrants`**：用户在 confirm 弹窗里选了"Allow once"（仅本会话）的路径集合；
//!   写入 layer-2 通过后，下次同一路径无需再 confirm。
//! - **`DraggedPaths`**：用户从终端拖入的文件/文件夹路径集合；同样仅会话有效，
//!   且仅作用于命中拖入路径前缀的操作。
//! - **`SessionPathRules`**：用户在当前 `pi chat` 会话里新增的 deny / readonly
//!   规则；写盘后立即进入内存 gate，避免必须重启才生效。
//!
//! 两者都用 `Arc<Mutex<HashSet<PathBuf>>>` 保证跨线程共享 + 内部可变。
//! 进程退出即清空（重启需重新授权）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::PathRule;

/// 会话级临时授权（write/edit/bash）。
#[derive(Debug, Clone, Default)]
pub struct SessionGrants {
    inner: Arc<Mutex<HashSet<PathBuf>>>,
}

impl SessionGrants {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一个授权路径（已规范化）。
    pub fn add(&self, path: PathBuf) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(path);
        }
    }

    /// 路径或其父目录是否在授权集中（前缀匹配，prefix 与 target 都规范化到最长存在祖先）。
    pub fn contains(&self, path: &Path) -> bool {
        let target = super::gate::canonicalize_with_existing_ancestor(path)
            .to_string_lossy()
            .to_string();
        match self.inner.lock() {
            Ok(g) => g.iter().any(|p| {
                let pc = super::gate::canonicalize_with_existing_ancestor(p);
                let prefix = pc.to_string_lossy();
                super::types::path_starts_with(&target, &prefix)
            }),
            Err(_) => false,
        }
    }

    /// 当前所有授权路径的快照（顺序无关）。
    pub fn snapshot(&self) -> Vec<PathBuf> {
        match self.inner.lock() {
            Ok(g) => g.iter().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// 拖入路径缓存（与 SessionGrants 同结构，但语义独立用于审计 `GrantSource`）。
#[derive(Debug, Clone, Default)]
pub struct DraggedPaths {
    inner: Arc<Mutex<HashSet<PathBuf>>>,
}

impl DraggedPaths {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, path: PathBuf) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(path);
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        let target = super::gate::canonicalize_with_existing_ancestor(path)
            .to_string_lossy()
            .to_string();
        match self.inner.lock() {
            Ok(g) => g.iter().any(|p| {
                let pc = super::gate::canonicalize_with_existing_ancestor(p);
                let prefix = pc.to_string_lossy();
                super::types::path_starts_with(&target, &prefix)
            }),
            Err(_) => false,
        }
    }

    pub fn snapshot(&self) -> Vec<PathBuf> {
        match self.inner.lock() {
            Ok(g) => g.iter().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// 会话级运行时 path_rules（deny / readonly）。
#[derive(Debug, Clone, Default)]
pub struct SessionPathRules {
    inner: Arc<Mutex<Vec<PathRule>>>,
}

impl SessionPathRules {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一条运行时规则。调用方负责先完成磁盘写入或确认这是临时规则。
    pub fn add(&self, rule: PathRule) {
        if let Ok(mut g) = self.inner.lock() {
            g.push(rule);
        }
    }

    /// 当前所有运行时规则快照，保留添加顺序。
    pub fn snapshot(&self) -> Vec<PathRule> {
        match self.inner.lock() {
            Ok(g) => g.clone(),
            Err(_) => Vec::new(),
        }
    }
}
