//! One inventory reconciliation for terminal commands and turn boundaries.
use std::time::{Duration, Instant};

use super::ChatContext;
use crate::ext::plugin::PluginCatalog;
use crate::AppError;

#[derive(Debug, Default)]
pub struct SyncTimings {
    pub lock_wait: Duration,
    pub discover_skills: Duration,
    pub discover_plugins: Duration,
    pub diff: Duration,
    pub retire: Duration,
    pub register: Duration,
    pub stop_vms: Duration,
    pub activate: Duration,
    pub total: Duration,
}

#[derive(Debug, Default)]
pub struct InventoryReport {
    pub skills_added: Vec<String>,
    pub skills_removed: Vec<String>,
    pub skills_changed: Vec<String>,
    pub plugins_added: Vec<String>,
    pub plugins_removed: Vec<String>,
    pub plugins_changed: Vec<String>,
    pub warnings: Vec<String>,
    pub timings: SyncTimings,
}

impl InventoryReport {
    pub fn has_changes(&self) -> bool {
        !self.skills_added.is_empty()
            || !self.skills_removed.is_empty()
            || !self.skills_changed.is_empty()
            || !self.plugins_added.is_empty()
            || !self.plugins_removed.is_empty()
            || !self.plugins_changed.is_empty()
    }
}

impl ChatContext {
    pub(crate) async fn sync_resource_inventory_before_turn(&self) -> Result<(), AppError> {
        let current = super::current_resource_inventory_epoch();
        if current
            <= self
                .session_runtime
                .seen_resource_inventory_epoch
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        self.sync_resource_inventory().await?;
        Ok(())
    }

    pub async fn sync_resource_inventory(&self) -> Result<InventoryReport, AppError> {
        let total = Instant::now();
        let lock = Instant::now();
        // Scanning is inside the scope lock too: an older scan cannot overwrite
        // inventory published by a later caller. JS activation remains outside.
        let sync_guard = self
            .scope_services
            .scope_container
            .inventory_sync
            .lock()
            .await;
        let mut report = InventoryReport::default();
        report.timings.lock_wait = lock.elapsed();
        if let Some(handle) = self
            .scope_services
            .skill_discovery_handle
            .lock()
            .await
            .take()
        {
            handle.abort();
            let _ = handle.await;
        }
        let config = self.config.clone();
        let root = self.scope_services.resource_root.clone();
        let (skills, plugins, skills_time, plugins_time) = tokio::task::spawn_blocking(move || {
            let phase = Instant::now();
            let skills = if config.skills.enabled {
                crate::core::skill::discover(&config, &root)
            } else {
                crate::core::skill::SkillSet::default()
            };
            let skills_time = phase.elapsed();
            let phase = Instant::now();
            let plugins = PluginCatalog::discover(&config, &root);
            (skills, plugins, skills_time, phase.elapsed())
        })
        .await
        .map_err(|error| AppError::Plugin(format!("inventory scan failed: {error}")))?;
        report.timings.discover_skills = skills_time;
        report.timings.discover_plugins = plugins_time;
        report.warnings.extend(skills.warnings.clone());
        let phase = Instant::now();
        let skill_read_failed = skills.warnings.iter().any(|warning| {
            warning == "skills_discovery_roots_failed"
                || warning.starts_with("skills_root_unreadable:")
                || warning.starts_with("skills_scan_unreadable:")
        });
        if !skill_read_failed {
            let old = self.skill_set_snapshot();
            if skills != old {
                for (name, skill) in &skills.by_name {
                    match old.by_name.get(name) {
                        None => report.skills_added.push(name.clone()),
                        Some(previous) if previous != skill => {
                            report.skills_changed.push(name.clone())
                        }
                        _ => {}
                    }
                }
                report.skills_removed = old
                    .by_name
                    .keys()
                    .filter(|name| !skills.by_name.contains_key(*name))
                    .cloned()
                    .collect();
                *self.scope_services.skill_set.write() = skills;
            }
        }
        let catalog = match plugins {
            Ok(catalog) => {
                report.warnings.extend(catalog.warnings.clone());
                report
                    .warnings
                    .extend(catalog.diagnostics.iter().map(|diagnostic| {
                        format!(
                            "plugin catalog ignored {}: {}",
                            diagnostic.path.display(),
                            diagnostic.reason
                        )
                    }));
                if catalog.unreadable {
                    None
                } else {
                    Some(catalog)
                }
            }
            Err(error) => {
                report
                    .warnings
                    .push(format!("plugin inventory unchanged: {error}"));
                None
            }
        };
        if let (Some(manager), Some(catalog)) = (&self.global_services.plugin_manager, &catalog) {
            for id in manager.list_loaded() {
                let Some(info) = manager.get_plugin(&id) else {
                    continue;
                };
                match catalog.get(&id) {
                    None if info.fingerprint.is_some() => report.plugins_removed.push(id),
                    Some(entry) if info.fingerprint != Some(entry.fingerprint) => {
                        report.plugins_changed.push(id)
                    }
                    _ => {}
                }
            }
            report.plugins_added = catalog
                .iter()
                .filter(|(id, _)| manager.get_plugin(id).is_none())
                .map(|(id, _)| id.clone())
                .collect();
        }
        let host_functions = catalog.as_ref().map(|catalog| {
            let (functions, warnings) =
                super::context::collect_host_functions_from_catalog(catalog);
            report.warnings.extend(warnings);
            functions
        });
        report.timings.diff = phase.elapsed();
        if let (Some(manager), Some(catalog)) =
            (&self.global_services.plugin_manager, catalog.as_ref())
        {
            if !report.plugins_removed.is_empty() || !report.plugins_changed.is_empty() {
                let phase = Instant::now();
                for id in report.plugins_removed.iter().chain(&report.plugins_changed) {
                    manager.retire_plugin(id)?;
                }
                report.timings.retire = phase.elapsed();
            }
            if !report.plugins_added.is_empty()
                || !report.plugins_changed.is_empty()
                || !report.plugins_removed.is_empty()
            {
                let phase = Instant::now();
                for id in report.plugins_added.iter().chain(&report.plugins_changed) {
                    let entry = catalog
                        .get(id)
                        .expect("diff IDs came from the same catalog");
                    for tool in super::context::register_catalog_entry(manager, entry)? {
                        self.global_services
                            .tool_registry
                            .register_tool(tool, id)
                            .await?;
                    }
                }
                self.global_services
                    .function_registry
                    .replace_all(host_functions.unwrap_or_default());
                report.timings.register = phase.elapsed();
            }
        }
        for values in [
            &mut report.skills_added,
            &mut report.skills_removed,
            &mut report.skills_changed,
            &mut report.plugins_added,
            &mut report.plugins_removed,
            &mut report.plugins_changed,
        ] {
            values.sort();
        }
        if report.has_changes() {
            super::publish_resource_inventory_change();
        }
        // Record the generation actually incorporated, not an epoch published by
        // another caller while this session activates its plugins outside the lock.
        let incorporated_epoch = super::current_resource_inventory_epoch();
        drop(sync_guard);
        if let (Some(manager), Some(session_id)) = (
            &self.global_services.plugin_manager,
            self.session_runtime.session.current_session_id()?,
        ) {
            let phase = Instant::now();
            manager.stop_stale_session_vms(&session_id).await?;
            report.timings.stop_vms = phase.elapsed();
            let phase = Instant::now();
            let (activated, warnings) =
                super::context::activate_session_plugins(manager.clone(), &session_id).await;
            report.warnings.extend(warnings);
            if activated {
                report.timings.activate = phase.elapsed();
            }
        }
        self.session_runtime
            .seen_resource_inventory_epoch
            .store(incorporated_epoch, std::sync::atomic::Ordering::Release);
        report.timings.total = total.elapsed();
        tracing::info!(target: "tomcat::inventory_sync", timings = ?report.timings,
            skills_added = report.skills_added.len(), skills_removed = report.skills_removed.len(),
            skills_changed = report.skills_changed.len(), plugins_added = report.plugins_added.len(),
            plugins_removed = report.plugins_removed.len(), plugins_changed = report.plugins_changed.len(),
            warnings = ?report.warnings, "resource inventory synchronized");
        Ok(report)
    }
}
