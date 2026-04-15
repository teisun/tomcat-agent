# Skills 与 Tools

**版本**：见 [README.md](README.md) 同步表；路径相对 **`openclaw/` 仓库根**。

## 零、先用大白话

**Skills** 像 **菜谱夹**：每道菜一张 `SKILL.md`，写步骤、要写啥命令。  
**Tools** 像 **遥控器实体键**：发消息、查记忆、开浏览器……模型只能按 **印出来的键** 按。  
哪些键能按，是 **策略 + 沙箱** 说了算，不是模型说了算。

**这一节你会学到**：技能从哪些目录合并；工具和 Gateway 啥关系。

---

## ASCII 核心四图

### 1) 结构图

```text
skills/（随包带的菜谱） + workspace/.agents/skills 等
        |
        v
agents/skills 解析、排序、过滤
        |
        v
Pi 工具表（gateway / sessions / memory / browser …）
```

### 2) 调用流图

```text
启动 / 热更
  -> 扫描 SKILL.md
      -> 生成 tool schema 或 prompt 片段
          -> 合并进 Agent 可见集合
              -> 模型发 toolCall -> src/agents/tools/* 执行
```

### 3) 时序图

```text
User        Skill loader        tool catalog       ClawdHub
  |              |                    |                |
  | 装技能        |                    |                |
  |------------->| register           |                |
  |              |（可选）拉远程清单  |--------------->|
```

### 4) 数据闭环图

```text
SKILL.md 改了
        |
        v
chokidar 触发刷新（可关）
        |
        v
下一轮对话工具表变
        |
        v
脚本产物写 workspace -> 再被 memory / 用户读到
```

---

## 一、Skills（菜谱从哪来）

多来源、**有优先级**；完整表格见 [19-目录结构详解.md](19-目录结构详解.md) 附录 Q3。  
**解析代码**：`src/agents/skills/workspace.ts` 等。  
**托管技能目录**：常见在 **`~/.openclaw/skills/`**（经 Gateway 安装）。

---

## 二、Tools（遥控器上的键）

| 方向 | 人话 | 代码入口 |
|------|------|----------|
| Gateway | 让模型「叫塔台办事」 | `src/agents/tools/gateway-tool.ts` |
| Sessions | 看别的会话、发消息 | `src/agents/tools/sessions-*-tool.ts` |
| Memory | 搜笔记、取片段 | `tool-catalog` + 嵌入式订阅（不单文件） |
| Browser | 自动化网页 | `src/agents/tools/` 下 browser 相关 |
| Cron / 图像 / 搜索 | 定时、生图、上网 | 同在 `src/agents/tools/`（以目录为准） |

**总装配**：`src/agents/pi-tools.ts`；**策略**：`src/agents/tool-policy.ts`。

---

## 三、和 Pi 的关系

Skills 最后也要变成 **模型能调的东西**（工具或提示）。  
ClawdHub 提供 **远程菜谱市场**；拉下来仍受 **白名单与沙箱** 约束。

---

## 常见误会

- **误会**：Skills 和 Tools 是同一张表。**正解**：Skills 多来自 **Markdown**；Tools 多是 **TypeScript 实现**；中间有适配层。  
- **误会**：模型看到工具就一定会用。**正解**：它会 **挑**；还可能被 **prompt 长度** 截断说明。  
- **误会**：关掉 browser tool 就绝对打不开网页。**正解**：还有 **渠道预览、别的入口**；安全要分层看。
