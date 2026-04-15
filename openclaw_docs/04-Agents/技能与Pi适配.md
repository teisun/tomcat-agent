# Agents 技能与 Pi 适配

## 零、先用大白话

Skills 像 **人类写给助理的菜谱**（`SKILL.md`）。  
Pi 适配层像 **翻译**：把菜谱里的步骤，变成模型看得懂的 **工具定义** 或 **提示片段**。

**这一节你会学到**：技能放哪；和「内置 tools」差在哪。

---

**设计思想**：Skill 由 SKILL.md 描述，可带脚本或服务。Workspace 技能、managed 技能、plugin 技能经 `agents/skills` 解析后，转换为 Pi 可用的 tool 定义或 prompt 注入。ClawdHub 提供技能注册表，支持自动搜索与拉取。

---

## ASCII 核心四图

### 1) 结构图

```text
skills/ 与 workspace 技能目录
        |
        v
agents/skills 解析 SKILL.md
        |
        v
Pi tools / system prompt 片段
```

### 2) 调用流图

```text
发现 SKILL.md
  -> 读取 frontmatter + 步骤
      -> 注册为 tool 或注入提示
          -> Agent 可见技能名被模型调用
```

### 3) 时序图

```text
User        Skill resolver       Pi Agent        ClawdHub
 |                |                  |               |
 | /skill         |                  |               |
 |--------------->| 拉元数据        |               |
 |                |------------------------------->|
 |                | 缓存 manifest   |               |
```

### 4) 数据闭环图

```text
本地 SKILL 变更
        |
        v
热重载技能索引
        |
        v
模型调用脚本产出写 workspace
        |
        v
下一轮对话读取更新后的 SKILL 输出
```

---

## 一、SKILL.md 与目录

- **内置示例目录**：仓库根 `skills/<name>/SKILL.md`（随发行打包的技能）。  
- **格式**：Markdown，可带 YAML frontmatter（`name`、`requires` 等）。  
- **解析**：`src/agents/skills/` 下解析 workspace / managed / plugin 等多来源（合并规则见 [19-目录结构详解.md](19-目录结构详解.md) 附录 Q3）。

---

## 二、Plugin Skills

- **plugin-skills**：从 plugins 加载的技能，与 workspace 技能合并。
- **tool 定义适配**：`pi-tool-definition-adapter` 将 skill 的 command 转为 Pi tool schema。

---

## 三、ClawdHub

- **技能注册表**：ClawdHub 提供可搜索的技能列表。  
- **拉取**：Agent 或用户可通过 `skills.install` 等 Gateway method 拉到 **`~/.openclaw/skills/`** 或 workspace（以配置为准）。

---

## 常见误会

- **误会**：装了技能就等于模型会用它。**正解**：还要看 **白名单、过滤、prompt 容量**；模型可能「装没看见」。  
- **误会**：SKILL.md 写啥都会执行。**正解**：危险动作仍走 **沙箱 / 审批**。  
- **误会**：技能和插件是一回事。**正解**：插件更大（可带 channel）；技能更像 **带说明书的工具包**。
