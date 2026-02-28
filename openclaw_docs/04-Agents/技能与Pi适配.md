# Agents 技能与 Pi 适配

**设计思想**：Skill 由 SKILL.md 描述，可带脚本或服务。Workspace 技能、managed 技能、plugin 技能经 `agents/skills` 解析后，转换为 Pi 可用的 tool 定义或 prompt 注入。ClawdHub 提供技能注册表，支持自动搜索与拉取。

---

## 一、SKILL.md 与目录

- **目录**：`openclaw/skills/<name>/SKILL.md`
- **格式**：Markdown，含 name、description、commands、tools 等元数据。
- **解析**：`openclaw/src/agents/skills/` 下的逻辑解析 SKILL.md，构建 workspace skills prompt 或 tool 定义。

---

## 二、Plugin Skills

- **plugin-skills**：从 plugins 加载的技能，与 workspace 技能合并。
- **tool 定义适配**：`pi-tool-definition-adapter` 将 skill 的 command 转为 Pi tool schema。

---

## 三、ClawdHub

- **技能注册表**：ClawdHub 提供可搜索的技能列表。
- **拉取**：Agent 或用户可通过 skills.install 等从 ClawdHub 拉取技能到 workspace。
