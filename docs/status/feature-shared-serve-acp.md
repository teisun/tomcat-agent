### feature/shared-serve-acp 状态入口

本分支的唯一状态表维护在 [tomcat/docs/status/feature-shared-serve-acp.md](../../tomcat/docs/status/feature-shared-serve-acp.md)，与现有状态汇总目录保持一致。元数据、Cov%、DONE、INTERFACE、验证记录和 BLOCKED 均以该文件为准，此处不维护副本。

本次提交将 write-plans 原样纳入默认内置技能，精简 PLAN 加载提醒，并让计划评审只保留独立审查职责；相关代码、测试、文档全部纳入。此前离线验证结果、完整库门禁未全绿的边界及既有 Windows 待验收事项记录在上述唯一状态表。本次提交按用户要求不重跑测试、构建或覆盖率，不安装、不重启共享服务、不推送；试验技能包、归档评测工具和历史产物继续保留原位，不纳入提交。
