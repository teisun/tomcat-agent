# `docs/` 文档地图

本目录存放 **tomcat** 的技术文档、蓝图规范、进度看板与用户说明。

## 目录结构

```text
docs/
├── README.md
├── INTEGRATION.md
├── TODOS.md
├── openspec/
├── architecture/
├── agents/
├── status/
└── user-guide.md
```

## 怎么读

### 1. 蓝图与规范

- [`openspec/specs/Architecture.md`](./openspec/specs/Architecture.md)：项目级架构蓝图与总索引。
- `openspec/specs/guides/`：写文档、写测试、写工作流的上位规范。

### 2. 模块级架构方案

- [`architecture/README.md`](./architecture/README.md)：`docs/architecture/` 内部阅读顺序与主题分组。
- [`architecture/project-overview-panorama.md`](./architecture/project-overview-panorama.md)：整体全景与主链路。
- [`architecture/plugin-system-overview.md`](./architecture/plugin-system-overview.md)：当前 rquickjs 插件体系入口。
- [`architecture/tools/`](./architecture/tools/)：内置工具冻结方案。

### 3. 角色卡、计划与任务板

- `agents/`：Dispatcher、任务板、计划模板与各类执行辅助文档。

### 4. 进度与用户说明

- `status/`：分支级进度碎片。
- [`user-guide.md`](./user-guide.md)：面向使用者的操作说明。

## 关键约定

| 约定 | 说明 |
|------|------|
| 进度碎片只写 `docs/status/` | 当前分支的进度只落在对应 status 文件中 |
| `docs/INTEGRATION.md` 禁止手改 | 由汇总命令自动生成 |
| `docs/architecture/` 只放模块级方案 | 总入口在 `openspec/specs/Architecture.md`，目录级入口在 `docs/architecture/README.md` |
| 架构文档默认父文档向下导航 | 非必要不在同级文档之间互相串链，避免形成多层阅读栈 |
