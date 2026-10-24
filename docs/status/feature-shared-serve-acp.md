### feature/shared-serve-acp 状态入口

本分支的唯一状态表维护在 [tomcat/docs/status/feature-shared-serve-acp.md](../../tomcat/docs/status/feature-shared-serve-acp.md)，与现有状态汇总目录保持一致。元数据、Cov%、DONE、INTERFACE、验证记录和 BLOCKED 均以该文件为准，此处不维护副本。

本次提交修复 VS Code 多根工作区文件引用丢失所属项目路径的问题；开发及 macOS 验证已完成，Windows 实机验收保留待完成并按用户要求暂停等待产物。本次提交不重跑测试，既有验证结果、接口边界及阻塞均记录在上述唯一状态表。
