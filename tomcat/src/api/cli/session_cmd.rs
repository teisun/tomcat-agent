//! `tomcat session` 子命令实现：在 scope（claw/code）内 list / new / switch /
//! delete / archive / search。

use crate::infra::i18n::tr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    resolve_model_thinking_path, resolve_sessions_dir, session_key_for_agent, AppConfig, AppError,
    ModelPrefsStore, SessionManager, SessionMode, ThinkingLevel,
};

use super::{resolve_default_cli_session_mode, SessionScopeArg, SessionSub};

struct SessionDisplayRow {
    session_id: String,
    key: String,
    is_current: bool,
}

pub(crate) fn run_session(sub: SessionSub, cfg: &AppConfig) -> Result<(), AppError> {
    match sub {
        SessionSub::List { scope } => {
            let mgr = scoped_session_manager(cfg, scope)?;
            let rows = session_display_rows(&mgr)?;
            if rows.is_empty() {
                println!("{}", tr("cli.session.empty", &[]));
                return Ok(());
            }
            for row in rows {
                let marker = if row.is_current { "*" } else { " " };
                println!("{} {}  {}", marker, row.session_id, row.key);
            }
        }
        SessionSub::New { scope } => {
            let mgr = scoped_session_manager(cfg, scope)?;
            let entry = mgr.new_current_session(None)?;
            println!(
                "{}",
                tr(
                    "cli.session.created",
                    &[
                        ("id", &entry.session_id),
                        ("key", mgr.current_session_key())
                    ]
                )
            );
        }
        SessionSub::Switch { session_id, scope } => {
            let mgr = scoped_session_manager(cfg, scope)?;
            match mgr.switch_current_to_session_id(&session_id) {
                Ok(_) => println!(
                    "{}",
                    tr(
                        "cli.session.switched",
                        &[("id", &session_id), ("key", mgr.current_session_key())]
                    )
                ),
                Err(AppError::Config(_)) => {
                    println!("{}", tr("session.not_found", &[("id", &session_id)]));
                }
                Err(e) => return Err(e),
            }
        }
        SessionSub::Delete { session_id, scope } => {
            let (mgr, mode) = scoped_session_manager_and_mode(cfg, scope)?;
            let deletion = mgr.begin_delete_session(&session_id)?;
            if deletion.exists() {
                cleanup_openai_files_for_session(
                    cfg,
                    mgr.sessions_dir(),
                    &session_id,
                    "session_delete",
                );
                cleanup_plugin_session_for_session(cfg, mode, &session_id, "session_delete");
            }
            let trail = crate::resolve_agent_trail_dir(cfg)?;
            let outcome = deletion.commit(Some(&trail))?;
            println!(
                "{}",
                crate::infra::i18n::tr("session.deleted", &[("id", &session_id)])
            );
            for warning in outcome.warnings {
                eprintln!(
                    "{}",
                    crate::infra::i18n::tr("session.cleanup_failed", &[("detail", &warning)])
                );
            }
        }
        SessionSub::Archive { session_id, scope } => {
            let (mgr, mode) = scoped_session_manager_and_mode(cfg, scope)?;
            cleanup_openai_files_for_session(
                cfg,
                mgr.sessions_dir(),
                &session_id,
                "session_archive",
            );
            cleanup_plugin_session_for_session(cfg, mode, &session_id, "session_archive");
            match mgr.archive_session(&session_id) {
                Ok(()) => println!("{}", tr("cli.session.archived", &[("id", &session_id)])),
                Err(AppError::Config(_)) => {
                    println!("{}", tr("session.not_found", &[("id", &session_id)]))
                }
                Err(e) => return Err(e),
            }
        }
        SessionSub::Search { query, scope } => {
            let mgr = scoped_session_manager(cfg, scope)?;
            let rows = session_display_rows(&mgr)?;
            if rows.is_empty() {
                println!("{}", tr("cli.session.searchEmpty", &[]));
                return Ok(());
            }
            let q = query.as_deref().unwrap_or("");
            for row in rows {
                let key_matches = row.key.contains(q);
                if q.is_empty() || key_matches || row.session_id.contains(q) {
                    println!("{}  {}", row.key, row.session_id);
                }
            }
        }
    }
    Ok(())
}

fn session_display_rows(mgr: &SessionManager) -> Result<Vec<SessionDisplayRow>, AppError> {
    let entries = mgr.list_sessions()?;
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let current_key = mgr.current_session_key().to_string();
    let current_entry = mgr.get_session(&current_key)?;
    let current_id = current_entry.as_ref().map(|entry| entry.session_id.clone());

    Ok(entries
        .into_iter()
        .map(|(session_id, _)| SessionDisplayRow {
            is_current: current_id.as_deref() == Some(session_id.as_str()),
            key: current_key.clone(),
            session_id,
        })
        .collect())
}

fn resolve_scope_mode(
    cfg: &AppConfig,
    scope: Option<SessionScopeArg>,
) -> Result<SessionMode, AppError> {
    match scope {
        Some(scope) => Ok(scope.into_mode()),
        None => resolve_default_cli_session_mode(cfg),
    }
}

fn scoped_session_manager(
    cfg: &AppConfig,
    scope: Option<SessionScopeArg>,
) -> Result<SessionManager, AppError> {
    scoped_session_manager_and_mode(cfg, scope).map(|(mgr, _)| mgr)
}

fn scoped_session_manager_and_mode(
    cfg: &AppConfig,
    scope: Option<SessionScopeArg>,
) -> Result<(SessionManager, SessionMode), AppError> {
    let sessions_path = resolve_sessions_dir(cfg)?;
    std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
    let mode = resolve_scope_mode(cfg, scope)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let session_key = session_key_for_agent(&cfg.agent.id, mode, &cwd);
    Ok((SessionManager::new_scoped(sessions_path, session_key), mode))
}

fn cleanup_openai_files_for_session(
    cfg: &AppConfig,
    sessions_dir: &std::path::Path,
    session_id: &str,
    reason: &str,
) {
    let model_catalog = match crate::core::llm::ModelCatalog::load(cfg) {
        Ok(v) => std::sync::Arc::new(v),
        Err(e) => {
            tracing::warn!(
                error = %e,
                session_id = session_id,
                "skip openai files cleanup: cannot load llm model catalog"
            );
            return;
        }
    };
    let default_reasoning = ThinkingLevel::parse_or_medium(&cfg.llm.thinking.level).0;
    let model_prefs = match resolve_model_thinking_path(cfg)
        .and_then(|path| ModelPrefsStore::load(path, default_reasoning))
    {
        Ok(store) => Arc::new(store),
        Err(error) => {
            tracing::warn!(
                error = %error,
                session_id,
                "skip openai files cleanup: cannot load model preferences"
            );
            return;
        }
    };
    let resolver =
        crate::core::llm::DefaultLlmResolver::new(cfg.clone(), model_catalog, model_prefs);
    let llm = match crate::core::llm::LlmResolver::resolve(
        &resolver,
        crate::core::llm::LlmScene::Main,
        None,
    ) {
        Ok(v) => v.provider_impl,
        Err(e) => {
            tracing::warn!(
                error = %e,
                session_id = session_id,
                "skip openai files cleanup: cannot resolve llm provider"
            );
            return;
        }
    };
    let Some(runtime) = crate::core::llm::openai_files::build_runtime_for_provider(
        llm.as_ref(),
        &cfg.llm.files,
        sessions_dir,
        session_id,
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
                session_id = session_id,
                "skip openai files cleanup: cannot build runtime"
            );
            return;
        }
    };
    let summary = rt.block_on(async { runtime.cleanup_registered_files(reason).await });
    if summary.failed > 0 {
        tracing::warn!(
            session_id = session_id,
            total = summary.total,
            deleted = summary.deleted,
            failed = summary.failed,
            "openai files cleanup finished with failures"
        );
    }
}

fn cleanup_plugin_session_for_session(
    _cfg: &AppConfig,
    _mode: SessionMode,
    session_id: &str,
    reason: &str,
) {
    if let Err(error) = crate::api::chat::end_cached_plugin_session(session_id) {
        tracing::warn!(error = %error, session_id, reason, "plugin session cleanup finished with failures");
    }
}
