# Skills 与 Tools

**设计思想**：Skills 目录约定与 SKILL.md 定义技能；Tools 为 Pi 暴露的 gateway、sessions、memory、browser 等，技能与 Pi 工具定义通过 plugin-skills 适配；ClawdHub 提供技能注册表与拉取。

---

## 一、Skills

- **目录约定**：`openclaw/skills/`，每个技能含 SKILL.md。
- **workspace**：`openclaw/src/agents/skills/workspace.ts`，解析 SKILL.md、workspace 技能。
- **plugin-skills**：插件技能与 tool 定义适配。
- **ClawdHub**：技能注册表与拉取流程。

---

## 二、Tools

- **gateway-tool**：调用 Gateway Methods。
- **sessions-***：sessions-spawn、sessions-send、sessions-list、sessions-history。
- **memory-tool**：记忆检索。
- **browser-tool**：浏览器自动化。
- **cron-tool**：定时任务。
- **image-tool**：图像处理。
- **web-fetch / web-search**：网络请求与搜索。

---

## 三、与 Pi 的适配

- **pi-tools.ts**：`openclaw/src/agents/pi-tools.ts`，工具定义与 policy。
- **tool-policy**：sandbox 白名单、tool-policy 控制暴露范围。
