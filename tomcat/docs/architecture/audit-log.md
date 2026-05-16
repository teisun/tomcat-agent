# 审计日志设计

本文档描述 tomcat 独立审计日志模块的技术方案，与基础设施层「日志与审计」对应，实现 T1-P1-001 审计日志系统完整落地。

## 1. 目标与原则

- **独立于业务日志**：审计日志与 tracing 业务日志分离，专用存储、专用格式，便于合规与追溯。
- **仅追加、不可篡改**：写入仅支持 append，不提供按条修改或删除单条记录的接口；清理按策略（保留 N 天）整体重写或按日切分删文件。
- **全链路覆盖**：4 原语、工具调用、Hostcall、插件生命周期（load/enable/disable/unload）等关键路径均写入审计。

**说明**：当前审计日志为明文存储；加密存储为后续 TODO。

## 2. 存储形态

- **路径**：由 `resolve_audit_dir(cfg)` 推导，为 `work_dir/agents/main/audit/`；审计文件名为 `audit.jsonl`。
- **格式**：每行一条 JSON（JSONL），字段包含 `timestamp`（ISO8601）与按类型区分的 payload（见下）。
- **写入**：仅通过 `AuditStore::append` 追加，使用 `OpenOptions::create(true).append(true)`，多线程通过 `Mutex` 保护。
- **目录创建**：`ensure_work_dir_structure` 在启动时创建 `audit` 子目录。

## 3. 配置

- `security.enable_audit_log`：是否启用审计（默认 true）；为 false 时使用 `TracingAuditRecorder`，不写文件。
- `security.audit_log_retention_days`：保留最近 N 天（默认 90）；用于清理策略，0 在校验时拒绝。

## 4. 数据结构（与实现一致）

- **AuditEntryRow**（存储格式）：`timestamp` + `payload`（枚举：primitive / tool_call / hostcall / plugin_lifecycle），无 id；id 在查询时按行号 1-based 赋予。
- **AuditEntry**（查询/展示）：在 Row 基础上增加 `id`，供 CLI list/show 与导出使用。
- **AuditFilter**：支持按时间（since/until）、类型（kind）、插件（plugin_id）、条数上限（limit）过滤。

## 5. 写入路径与 AuditRecorder

- **AuditRecorder** trait：`record_primitive`、`record_tool_call`、`record_hostcall`、`record_plugin_lifecycle`。
- **TracingAuditRecorder**：仅输出到 tracing，不落盘。
- **FileAuditRecorder**：将各 *Entry 转为 `AuditEntryRow`（含当前时间戳），调用 `AuditStore::append`。
- **注入点**：当 `enable_audit_log == true` 时，CLI（build_plugin_context）与 ChatContext 使用 `AuditStore::open_if_enabled` 得到 `AuditStore`，再构造 `FileAuditRecorder`，注入到 `DefaultPrimitiveExecutor`、`DefaultToolRegistry`、`HostApiDispatcher`（with_audit）、`PluginManager`（set_audit_recorder）。
- **checkpoint restore 审计**：`tomcat chat` 本地命令 `/restore` 成功或失败时，额外通过 `record_hostcall(module="session", method="restore")` 记一条宿主审计，便于串起 transcript 中的 `Custom{checkpoint.restore}` 与磁盘回滚动作。

## 6. 查询、导出与清理

- **query(filter)**：按行读取 JSONL，解析为带 id 的 `AuditEntry`，应用 filter，按时间倒序后截断 limit；用于 list/show。
- **export_to(path)**：将全量查询结果序列化为 JSON 数组，通过 `write_file_atomic` 写入指定路径。
- **cleanup_retention(days)**：按时间戳解析每条记录，保留在截止日期之后的记录，重写文件时使用原子替换（写临时文件再 rename），避免损坏。

CLI `tomcat audit list` 执行前可选调用 `store.cleanup()`，按配置的保留天数清理。

## 7. CLI 对接

- `tomcat audit list [--limit N]`：使用 `AuditStore::query`，输出序号、时间、类型、状态、详情。
- `tomcat audit show <id>`：按 id（行号）从 query 结果中取单条展示。
- `tomcat audit export <path>`：调用 `AuditStore::export_to`。
- 当 `enable_audit_log == false` 或审计目录不存在时，提示友好信息并正常退出。

## 8. 与工作目录布局的关系

审计目录 `work_dir/agents/main/audit` 与 `logs`、`sessions`、`checkpoints`、`plugins`、`tmp`、`workspace` 并列，约定见 [工作目录与数据布局](work-dir-and-data-layout.md)。若后续引入按日切分（如 `audit_2025-03-13.jsonl`），清理可改为删除过期文件，查询则多文件合并或按日过滤。
