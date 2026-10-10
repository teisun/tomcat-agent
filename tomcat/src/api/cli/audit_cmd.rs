//! `tomcat audit` 子命令实现：list / show / export。

use crate::infra::i18n::tr;
use std::path::PathBuf;

use crate::{resolve_audit_dir, wire, AppConfig, AppError, AuditFilter, AuditStore};

use super::AuditSub;

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct AuditDisplayEntry {
    pub(crate) index: usize,
    pub(crate) timestamp: String,
    pub(crate) audit_type: String,
    pub(crate) detail: String,
    pub(crate) success: String,
}

#[allow(dead_code)]
pub(crate) fn parse_audit_line(line: &str, index: usize) -> Option<AuditDisplayEntry> {
    let audit_type = if line.contains("audit primitive") {
        wire::WIRE_AUDIT_PRIMITIVE
    } else if line.contains("audit tool_call") {
        wire::WIRE_TOOL_CALL
    } else if line.contains("audit hostcall") {
        wire::WIRE_AUDIT_HOSTCALL
    } else {
        return None;
    };

    let timestamp = line
        .find(char::is_numeric)
        .and_then(|start| line.get(start..start + 30.min(line.len() - start)))
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("unknown")
        .to_string();

    let success = if line.contains("success=true") || line.contains("success: true") {
        "OK"
    } else if line.contains("success=false") || line.contains("success: false") {
        "FAIL"
    } else {
        "?"
    };

    let detail = line
        .find("operation=")
        .or_else(|| line.find("tool_name="))
        .or_else(|| line.find("module="))
        .map(|start| line[start..].chars().take(80).collect::<String>())
        .unwrap_or_else(|| {
            let trimmed = line.trim();
            trimmed.chars().take(80).collect::<String>()
        });

    Some(AuditDisplayEntry {
        index,
        timestamp,
        audit_type: audit_type.to_string(),
        detail,
        success: success.to_string(),
    })
}

#[allow(dead_code)]
fn find_latest_log_file(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .max_by_key(|p| {
            p.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

#[allow(dead_code)]
pub(crate) fn read_audit_entries(
    log_path: &std::path::Path,
    limit: Option<u32>,
) -> Result<Vec<AuditDisplayEntry>, AppError> {
    use std::io::BufRead;
    let file = std::fs::File::open(log_path).map_err(AppError::Io)?;
    let reader = std::io::BufReader::new(file);
    let mut entries = Vec::new();
    let mut audit_index = 0usize;
    for line in reader.lines() {
        let line = line.map_err(AppError::Io)?;
        if let Some(entry) = parse_audit_line(&line, audit_index) {
            audit_index += 1;
            entries.push(entry);
        }
    }
    entries.reverse();
    let max = limit.unwrap_or(50) as usize;
    entries.truncate(max);
    Ok(entries)
}

pub(crate) fn run_audit(sub: AuditSub, cfg: &AppConfig) -> Result<(), AppError> {
    if !cfg.security.enable_audit_log {
        println!("{}", tr("cli.audit.disabled", &[]));
        return Ok(());
    }
    let store = match AuditStore::new(cfg) {
        Ok(s) => s,
        Err(e) => {
            println!(
                "{}",
                tr("cli.audit.openFailed", &[("detail", &e.to_string())])
            );
            return Ok(());
        }
    };
    let audit_dir = resolve_audit_dir(cfg)?;
    if !audit_dir.exists() {
        println!(
            "{}",
            tr(
                "cli.audit.dirMissing",
                &[("path", &audit_dir.display().to_string())]
            )
        );
        return Ok(());
    }

    match sub {
        AuditSub::List { limit } => {
            let _ = store.cleanup();
            let filter = AuditFilter {
                limit: limit.or(Some(50)),
                ..Default::default()
            };
            let entries = store.query(&filter)?;
            if entries.is_empty() {
                println!("{}", tr("cli.audit.empty", &[]));
                return Ok(());
            }
            println!(
                "{:<6} {:<28} {:<14} {:<6} {}",
                tr("cli.label.index", &[]),
                tr("cli.label.time", &[]),
                tr("cli.label.type", &[]),
                tr("cli.label.status", &[]),
                tr("cli.label.detail", &[])
            );
            println!("{}", "-".repeat(90));
            for e in &entries {
                let status = if e.success() { "OK" } else { "FAIL" };
                println!(
                    "{:<6} {:<28} {:<14} {:<6} {}",
                    e.id,
                    e.timestamp,
                    e.kind_label(),
                    status,
                    e.detail_short()
                );
            }
            println!(
                "{}",
                tr("cli.audit.count", &[("count", &entries.len().to_string())])
            );
        }
        AuditSub::Show { id } => {
            let idx: u64 = id.parse().unwrap_or(0);
            let filter = AuditFilter {
                limit: None,
                ..Default::default()
            };
            let entries = store.query(&filter)?;
            match entries.iter().find(|e| e.id == idx) {
                Some(e) => {
                    let status = if e.success() { "OK" } else { "FAIL" };
                    println!("{}:   {}", tr("cli.label.index", &[]), e.id);
                    println!("{}:   {}", tr("cli.label.time", &[]), e.timestamp);
                    println!("{}:   {}", tr("cli.label.type", &[]), e.kind_label());
                    println!("{}:   {}", tr("cli.label.status", &[]), status);
                    println!("{}:   {}", tr("cli.label.detail", &[]), e.detail_short());
                }
                None => {
                    println!("{}", tr("cli.audit.notFound", &[("id", &id)]));
                }
            }
        }
        AuditSub::Export { path } => {
            let filter = AuditFilter {
                limit: None,
                ..Default::default()
            };
            let entries = store.query(&filter)?;
            if entries.is_empty() {
                println!("{}", tr("cli.audit.exportEmpty", &[]));
                return Ok(());
            }
            store.export_to(&path)?;
            println!(
                "{}",
                tr(
                    "cli.audit.exported",
                    &[
                        ("count", &entries.len().to_string()),
                        ("path", &path.display().to_string())
                    ]
                )
            );
        }
    }
    Ok(())
}
