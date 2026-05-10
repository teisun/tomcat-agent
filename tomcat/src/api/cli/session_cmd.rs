//! `tomcat session` 子命令实现：list / new / switch / delete / archive / search。

use crate::{resolve_sessions_dir, AppConfig, AppError, SessionManager};

use super::SessionSub;

pub(crate) fn run_session(sub: SessionSub, cfg: &AppConfig) -> Result<(), AppError> {
    let sessions_path = resolve_sessions_dir(cfg)?;
    std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
    let mgr = SessionManager::new(sessions_path.clone());
    match sub {
        SessionSub::List => {
            let list = mgr.list_sessions()?;
            if list.is_empty() {
                println!("当前无会话。使用 session new 创建。");
                return Ok(());
            }
            for (key, entry) in list {
                println!("{}  {}  {}", key, entry.session_id, entry.updated_at);
            }
        }
        SessionSub::New => {
            let key = mgr.current_session_key();
            let entry = mgr.create_session(key, None)?;
            println!("已创建会话: {}  {}", entry.session_id, key);
        }
        SessionSub::Switch { key } => {
            if mgr.get_session(&key)?.is_none() {
                println!("会话不存在: {}", key);
                return Ok(());
            }
            println!("当前会话 key 固定为 agent:main:main，切换逻辑占位。");
        }
        SessionSub::Delete { key } => {
            cleanup_openai_files_for_session(cfg, sessions_path.as_path(), &key, "session_delete");
            mgr.delete_session(&key)?;
            println!("已删除会话: {}", key);
        }
        SessionSub::Archive { key } => {
            cleanup_openai_files_for_session(cfg, sessions_path.as_path(), &key, "session_archive");
            mgr.archive_session(&key)?;
            println!("已归档会话: {}", key);
        }
        SessionSub::Search { query } => {
            let list = mgr.list_sessions()?;
            if list.is_empty() {
                println!("无会话");
                return Ok(());
            }
            let q = query.as_deref().unwrap_or("");
            for (key, entry) in list {
                if q.is_empty() || key.contains(q) || entry.session_id.contains(q) {
                    println!("{}  {}", key, entry.session_id);
                }
            }
        }
    }
    Ok(())
}

fn cleanup_openai_files_for_session(
    cfg: &AppConfig,
    sessions_dir: &std::path::Path,
    session_key: &str,
    reason: &str,
) {
    let llm = match crate::core::llm::resolve_llm(&cfg.llm) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                session_key = session_key,
                "skip openai files cleanup: cannot resolve llm provider"
            );
            return;
        }
    };
    let Some(runtime) = crate::core::llm::openai_files::build_runtime_for_provider(
        llm.as_ref(),
        &cfg.llm.files,
        sessions_dir,
        session_key,
    ) else {
        return;
    };

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                session_key = session_key,
                "skip openai files cleanup: cannot build runtime"
            );
            return;
        }
    };
    let summary = rt.block_on(async { runtime.cleanup_registered_files(reason).await });
    if summary.failed > 0 {
        tracing::warn!(
            session_key = session_key,
            total = summary.total,
            deleted = summary.deleted,
            failed = summary.failed,
            "openai files cleanup finished with failures"
        );
    }
}
