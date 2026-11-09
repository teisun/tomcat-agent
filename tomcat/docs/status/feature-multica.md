| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat | 2026-11-09 18:37 +0800 | ACTIVE | feature/multica | - |

### 集成概况

- 本入口供现有 monorepo 集成看板读取；完整实施、历史失败与验收证据见[详细记录](../../../docs/status/feature-multica.md)，不在两处复制测试清单。
- 交付范围：ACP 启动策略与旁观答题、计划交接、重试/缓存键、CLI 发现和公司源码适配包；仅合入本地 develop，保留历史，不推送、发布、安装或重启正式服务。
- 原 16 个提交已含 cli-discovery；审查补漏已在来源分支提交，门禁暴露的会话锁补漏聚焦通过，最终离线门禁仍进行中。尚未执行 develop 合并；覆盖率未测，不代签其它平台或公司二开环境。
