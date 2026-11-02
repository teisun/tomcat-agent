# Paperclip 接入验收清单

## A. 代码合并

- [ ] 公司分支基线、Node/pnpm 版本和 ACPX patch 已记录。
- [ ] `new-files/` 无同名覆盖地导入。
- [ ] `existing-files.patch` 已人工审查并应用。
- [ ] `tests/` 新增文件和现有测试补丁已导入。
- [ ] `tomcat_local` 在 shared/server/CLI/UI 的注册完整。
- [ ] ACPX command identity、`session/load` 兼容名单、skills 清理和 continuation 注册未遗漏。
- [ ] lockfile 是公司包管理器重新生成或审查后的结果，不是盲目覆盖。

## B. Tomcat 执行环境

- [ ] 执行机器安装了包含共享服务和 ACP 的 Tomcat 构建。
- [ ] `tomcat acp --help` 包含 `--skills-dir`。
- [ ] 运行 `tomcat serve status` 的账号与 Paperclip 执行账号一致。
- [ ] `tomcat init`、模型、Key 和 `llm.proxy` 已在该账号配置。
- [ ] Tomcat 任务目录是执行机器上的现有绝对路径。
- [ ] 没有把 Paperclip API key 当作模型 Key，也没有把真实 Key 放进仓库。

## C. 受控 Paperclip 旅程

| 场景 | 必须看到 |
| --- | --- |
| readiness | 只检查 `--version`、`acp --help`、`serve status`；不创建 session、不发模型请求。 |
| 新任务 | cwd、AGENTS/skills、模型和工具请求进入正确执行目录。 |
| 权限拒绝 | `deny-all` 下拒绝写入，目标文件不存在。 |
| 取消 | 只取消当前 Paperclip 任务；不是停止整个共享服务；实际转发进程退出。 |
| 继续 | 复用条件满足时走 `session/load`；不满足时明确 fresh 或失败。 |
| 技能清理 | 任务结束后本轮私有技能目录被清理，宿主共享目录不残留。 |
| 结果 | Paperclip 任务状态、输出、usage partial/unpriced 语义正确。 |

## D. 公司分支边界

- 受控模型验收与付费模型验收分开记录。
- Paperclip 的执行端、Tomcat ACP 转发器、共享服务和 workspace 必须在同一可访问环境。
- 任何公司特有容器、权限、队列、审计、代理或工作树定制都要单独覆盖。
- 未完成真实公司分支旅程前，不要写“公司 Paperclip 已接入成功”。
