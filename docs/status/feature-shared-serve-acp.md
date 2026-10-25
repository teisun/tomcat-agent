### feature/shared-serve-acp 状态入口

本分支的唯一状态表维护在 [tomcat/docs/status/feature-shared-serve-acp.md](../../tomcat/docs/status/feature-shared-serve-acp.md)，与现有状态汇总目录保持一致。元数据、Cov%、DONE、INTERFACE、验证记录和 BLOCKED 均以该文件为准，此处不维护副本。

本次提交修复临时 LLM 故障分类和按任务累计重试额度的问题，并加入五分钟自动 Retry/Resume 及完整整改；相关代码、测试、文档全部纳入。此前 macOS 验证结果、接口边界及既有 Windows 待验收事项记录在上述唯一状态表。本次提交按用户要求不重跑测试或覆盖率，不安装、不重启共享服务、不推送。
