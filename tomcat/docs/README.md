# docs/ 文档地图

本目录包含项目的**技术文档、进度跟踪、分享材料**等面向人阅读的文档。

---

## 目录结构

```text
docs/
├── README.md                          ◄── 本文件
├── INTEGRATION.md                     ◄── 集成进度看板（由 /aggregate-status 自动汇总）
├── TODOS.md                           ◄── 灵感/想法/待办收集池（按领域分类 + P0-P5 优先级）
│
├── openspec/                          ◄── 法律与蓝图（Constitution、Architecture、规范）
├── architecture/                      ◄── 模块级技术方案（AgentLoop、上下文、Plan、工具、插件、权限等）
│   ├── project-overview-panorama.md       全景与分层
│   ├── agent-loop.md / context-management.md / plan-runtime.md …
│   ├── tools/                             内置工具冻结方案（read/write/bash/plan …）
│   └── plugin-system/                   插件 / Wasm / hostcall 等
├── agents/                            ◄── Agent 角色卡、看板、计划模板（Dispatcher、TASK_BOARD）
│
├── status/                            ◄── 各分支进度碎片
│   ├── develop.md                         集成分支进度
│   ├── feature-async-hostcall.md          功能分支进度
│   ├── feature-cli-chat.md
│   ├── feature-js-api-alignment.md
│   └── feature-long-lived-vm.md
│
│
└── user-guide.md
```

运行时工作区目录树（`~/.tomcat/`）不在 `docs/` 下，见 [directory-structure.md](architecture/directory-structure.md)（与 [work-dir-and-data-layout](architecture/work-dir-and-data-layout.md) 配套）。

---

## 关键约定

| 约定 | 说明 |
|------|------|
| **进度碎片写入** | 仅写入 `docs/status/` 下对应当前 Git 分支的文件（规则见 [STATUS_GUIDE](openspec/specs/guides/workflow/STATUS_GUIDE.md)） |
| **集成看板生成** | `docs/INTEGRATION.md` 由 `/aggregate-status` 命令在 develop 分支上自动汇总，**禁止手动编辑** |
| **禁止新建平行 status** | 不要在 `docs/` 外或 `docs/agents/` 下再建 `status/` 目录 |
| **模块技术文档** | 新增或修订与 `src/` 模块对应的说明时，写在对应子目录的 `README.md`；索引用仓库根 [src/README.md](../src/README.md)；**已废弃** `docs/technical/` |

---

## 与其他目录的关系

```text
docs/openspec/      ──► 法律与蓝图（Constitution、Architecture、规范、变更）
docs/architecture/  ──► 模块级技术方案（与 openspec 配套，偏实现与冻结设计）
docs/agents/        ──► 谁按什么剧本干活（Dispatcher、角色卡、TASK_BOARD）
docs/               ──► 给人读的说明、进度、架构文档（本目录）
.cursor/            ──► 编辑器命令与规则（仓库根，与 docs/ 并列）
```
