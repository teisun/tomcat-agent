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
├── status/                            ◄── 各分支进度碎片
│   ├── develop.md                         集成分支进度
│   ├── feature-async-hostcall.md          功能分支进度
│   ├── feature-cli-chat.md
│   ├── feature-js-api-alignment.md
│   └── feature-long-lived-vm.md
│
├── sharing/                           ◄── 组内技术分享
│   ├── claw-agent-design.md               CLAW 核心设计理念 & 生产级 AI Agent
│   └── spec-driven-agent-workflow.md      规范驱动的 AI Agent 自动化工作流
│
├── reports/                           ◄── 测试与验收报告
│   └── nibbles_4.3_e2e_recap_report.md
│
└── user-guide.md
```

运行时工作区目录树（`~/.tomcat/`）不在 `docs/` 下，见 [directory-structure.md](../docs/architecture/directory-structure.md)（与 [work-dir-and-data-layout](../docs/architecture/work-dir-and-data-layout.md) 配套）。

---

## 关键约定

| 约定 | 说明 |
|------|------|
| **进度碎片写入** | 仅写入 `docs/status/` 下对应当前 Git 分支的文件（规则见 [STATUS_GUIDE](../openspec/specs/guides/workflow/STATUS_GUIDE.md)） |
| **集成看板生成** | `docs/INTEGRATION.md` 由 `/aggregate-status` 命令在 develop 分支上自动汇总，**禁止手动编辑** |
| **禁止新建平行 status** | 不要在根目录或 `agents/` 下再建 `status/` 目录 |
| **模块技术文档** | 新增或修订与 `src/` 模块对应的说明时，写在对应子目录的 `README.md`；索引用仓库根 [src/README.md](../src/README.md)；**已废弃** `docs/technical/` |

---

## 与其他目录的关系

```text
openspec/     ──► 法律与蓝图（Constitution、Architecture、规范、变更）
agents/       ──► 谁按什么剧本干活（Dispatcher、角色卡、TASK_BOARD）
docs/         ──► 给人读的说明、进度、分享、报告（本目录）
.cursor/      ──► 编辑器命令与规则
```
