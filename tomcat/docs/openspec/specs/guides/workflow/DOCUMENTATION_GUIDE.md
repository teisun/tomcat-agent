# 技术文档编写规范（索引）

`tomcat` 的技术文档分两类，分别有独立规范，**本页只做导航**。

| 类别 | 落盘位置 | 规范 | 标杆案例 |
|------|----------|------|----------|
| **A. 模块 README** | `src/**/README.md` | [`MODULE_README_SPEC.md`](MODULE_README_SPEC.md) | 各子模块 README + [`src/README.md`](../../../../src/README.md) 顶层索引 |
| **B. 架构方案** | `docs/architecture/**/*.md` | [`ARCHITECTURE_SPEC.md`](ARCHITECTURE_SPEC.md) | [`architecture/tools/search_files.md`](../../../../docs/architecture/tools/search_files.md) · [`architecture/tools/read.md`](../../../../docs/architecture/tools/read.md)（**§4.1 / §4.2** 已定稿选型与实施范例）· [`architecture/interrupt-and-cancellation.md`](../../../../docs/architecture/interrupt-and-cancellation.md)（**§1.3**：§4.1/§4.2 职责映射范例） |

## 二选一速判

```text
              ┌────────────────────────────────────┐
              │ 这次改动……                          │
              └──────────────┬─────────────────────┘
                             │
        ┌────────────────────┼─────────────────────┐
        ▼                    ▼                     ▼
  仅在某个 src/ 子目录   跨 ≥ 2 个 src/ 子目录   引入新的 wire / 状态机 /
  内部，对外行为不变     ，或新增/改动协议       生命周期 / 取消传播
        │                    │                     │
        ▼                    ▼                     ▼
   更新 src/<path>/    新增或更新 openspec/    新增或更新 openspec/
   README.md           specs/architecture/     specs/architecture/
   （A 类）             下对应的 *.md（B 类）   下对应的 *.md（B 类）
```

边界澄清：

- **A 描述"本模块做什么"，B 描述"一件事跨多个模块怎么串起来做"**。
- 同一议题若同时出现在 A 与 B 中，**以 B 为权威**；A 加回链不重复内容。
- 用户指南（`docs/user-guide.md`）、status 进度文件（`docs/status/`）**不替代** A / B；代码改动若影响对外行为或模块边界，应同步更新对应 README 或方案。

## 共享写作风格（A / B 共用）

| 项 | 要求 |
|----|------|
| 精准引用 | 文件 / 符号 / 行号尽量带路径；跨文件用 Markdown 相对链接 |
| 术语统一 | 全文用术语表里钉死的那个词，不允许同义词混用 |
| 时间点钉死 | "LLM 回复后" 一类模糊词必须在术语表或首次出现处用一句话约定指代位置 |
| Why 优先 | 先说"为什么"，再说"怎么做"；常识性技巧不展开 |
| ASCII 优先 | 概览图首选 ASCII（易 diff、终端友好）；复杂时序可补 mermaid |
| 表格优先 | 选型 / 状态转移 / 配置 / 风险 / 验收等结构化内容用表格 |
| 说人话 / 决策（B 类） | **先专业、后口语**：主要小节与图后可跟短段「说人话」；**§4.1** 七列矩阵第三列 **`决策`**（专业裁决结论）+ 末列 **`说人话`**（口语扫读）；其他高密度表末列 **`说人话`**：**SHOULD**，见 [`ARCHITECTURE_SPEC.md`](ARCHITECTURE_SPEC.md) **§4.1 / §14.1**；与 Constitution **二.10** 同向 |
| 与实现一致 | 实施期调整就地修订并标 `【未改签名 / 依赖 Drop】` 等显式标签；禁止留"原计划 / 现落地"两份真相 |
| 验收可执行 | 目标可量化、验收映射到具体测试函数名 |
| 选型与交付钉死 | B 类须在 **§4** 含 **§4.1 落地选型决策表（维度取舍，七列：含 `决策` + `说人话`）** + **§4.2 实施点（已闭环；交付物与代码落点在此表）**（[`ARCHITECTURE_SPEC.md`](ARCHITECTURE_SPEC.md) MUST）；写法见 [`read.md` §4.1–§4.2](../../../../docs/architecture/tools/read.md)；补充文档的 §4 映射见 [`interrupt-and-cancellation.md` §1.3](../../../../docs/architecture/interrupt-and-cancellation.md) |
