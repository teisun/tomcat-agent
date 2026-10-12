# Tomcat for VS Code

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.zh.md">简体中文</a>
</p>

Tomcat Agent Box 将 `tomcat serve --stdio` 运行时带到了专属的 VS Code 侧边栏中。

![Tomcat Agent Box screenshot](../assets/tomcat-agent-box.png)

这个扩展仅适用于 **VS Code**。

## 下载选择

本版本的首选安装渠道是 GitHub Release。

| 你想要什么 | 下载内容 | 适合谁 |
| --- | --- | --- |
| 最快上手、步骤最少 | `tomcat-vscode-ext-0.1.3-darwin-arm64.vsix` / `darwin-x64` / `linux-x64` | 想同时获得 Tomcat Agent Box **和** 配套 CLI 的新用户 |
| 我已经安装 CLI | `tomcat-vscode-ext-0.1.3.vsix` | 只想安装 VS Code 扩展的现有 CLI 用户 |
| 仅 CLI | `tomcat-cli-v0.1.8-<target>.tar.gz` | 只在终端使用、不需要 VS Code 扩展的用户 |

如果你不确定怎么选，就选适合你机器的 **platform-specific bundled VSIX**。

## 在 VS Code 中安装

1. 从 GitHub Release 下载合适的 `.vsix` 文件。
2. 安装它：

   ```bash
   code --install-extension /path/to/tomcat-vscode-ext-0.1.3-darwin-arm64.vsix --force
   ```

3. Reload VS Code。

安装之后会发生什么：

```text
Downloads/tomcat-vscode-ext-0.1.3-*.vsix
    -> VS Code 安装扩展
    -> VS Code 将其解压到扩展目录
    -> bundled CLI 位于已安装的扩展内部
```

bundled CLI **不会** 从你的 Downloads 文件夹直接运行。

## 打开 Tomcat Agent Box

1. 按 `Cmd/Ctrl+Shift+P`。
2. 运行 `Tomcat: Focus Agent Box`。
3. VS Code 会展开 `Secondary Side Bar` 并聚焦到 Tomcat Agent Box。

你也可以自己打开 `Secondary Side Bar`，然后点击 Tomcat Agent Box 图标。

## 首次设置

如果这是你第一次使用 Tomcat，扩展会引导你完成初始化：

1. 打开 Tomcat Agent Box。
2. 如果 Tomcat 还没有完成初始化，点击 `Start Setup`。
3. VS Code 会打开集成终端并帮你运行 `tomcat init`。
4. 完成提示后，如果 Tomcat 没有自动重新连接，就点击 `I've Finished Setup`。

示例首条消息：

```text
help me understand this repository
```

Tomcat Agent Box 默认会恢复当前项目的活动会话。使用面板顶部的 session picker 可以切换会话或创建新会话。现在模型选择器里还带有 `Add Models...` 入口，可直接打开专门的设置中心，在 VS Code 内新增自定义 OpenAI-compatible 或 Anthropic 模型。

当 Tomcat 提出澄清问题时：

- 问题固定显示在 Composer 上方的 **Needs your answer** 区域，并且没有超时；
- 其他会话有未回答问题时，session picker 会显示数量 badge；
- 切会话或仅重载 Webview 不会丢失未提交选择；
- 回答、整单跳过、中断、宿主断开会生成不同的历史卡片。VS Code Window / Extension Host reload 属于不可恢复断开，而仅重载 Webview 仍会继续等待。

## 会话、Settings 与计划审查

```text
会话下拉 -> 置顶/取消置顶 | 垃圾桶 -> 确认 -> 永久删除本地记录
顶部齿轮 -> Settings -> 通用 -> 语言
新建计划 -> 先保存文件 -> 审查计划 | 跳过，直接打开
```

### 置顶和删除会话

鼠标悬浮会话行，或用键盘聚焦行内按钮，可以看到图钉和垃圾桶图标；每个按钮都有悬浮提示和无障碍名称。置顶保存在当前 VS Code 工作区，不改变会话最后活跃时间，也不影响 CLI 排序。 保存失败会报错，不自动回滚；可重载窗口确认实际保存的状态。置顶保存不会阻塞会话删除。

删除需要确认，**不会替你停止正在运行的任务**；请先停止使用该会话的任务，包括其他进程中的任务。删除成功会移除目标会话的本地记录、专属 sidecar 和预热缓存、待办草稿、文件恢复基线，以及扩展中的输入草稿、队列和置顶。它**不撤销工作区代码、不删除计划文件、不擦除审计/调试轨迹，也不承诺删除共享缓存、共享 checkpoint 或远端上传文件**。其他会话使用的共享附件会保留。清理警告表示会话记录已删除，但可能有专属文件未清理。回复丢失属于“结果未知”：在同一 scope 的磁盘列表确认前保留本地状态，不能把断连当成删除失败。

### 语言与 Settings

顶部齿轮进入 **Settings → 通用**；会话忙碌或模型不支持工具时也能打开。原来的手动压缩按钮已移除，自动压缩和终端 `/compact` 仍保留。

可选 **自动**、**简体中文**、**English**。它们共用 `~/.tomcat/tomcat.config.toml` 中的 `ui.language = auto | zh-CN | en`：

- 自动模式下，扩展跟随 VS Code，独立 CLI 跟随系统语言。
- 当前窗口保存后，Host、四个 webview 和 Serve 原地切换；其他已打开的窗口或 CLI 下次启动读取。
- 保留输入草稿、表单未保存值及用户/模型原文；已经产生的时间线通知不回溯重译。
- VS Code 命令面板、菜单等静态贡献点仍跟随 **VS Code 显示语言**，不能由应用语言独立热切换。
- 环境变量 `TOMCAT__UI__LANGUAGE` 优先于磁盘偏好，Settings 会提示覆盖状态；也可在独立终端执行 `tomcat config set ui.language en` 修改同一份偏好。

Chat、Plan 等技术术语和模型 ID 可以保留英文；给模型的工具指导、开发诊断保持英文，第三方描述和外部原始诊断按原文显示。
`models.toml` 中显式填写的描述按原文显示。新初始化不再把产品描述复制进配置；内置模型只要没有显式描述，即使覆盖了其他参数，也会使用界面词库。既有用户文件不做迁移。
保存复用现有配置写入流程：其他配置值会保留，但注释和排版可能变化。

### 选择是否审查计划

新计划先保存，再在现有问答卡里询问是否审查。独立审查会检查设计、依赖、验收遗漏和过度设计，并可能直接完善同一份计划。文案说明：**可能需要几分钟或更久，并消耗额外模型用量**，不保证固定完成时间。

选择“跳过，直接打开”不会启动 Reviewer；不回答时持续等待，不超时也不自动审查。Stop 或宿主断连不会删除已经保存的计划。跳过不代表计划已通过或已完成，审查也不会直接启动执行。

## 可选设置

大多数用户 **不需要** 手动配置任何内容。

只有在你想覆盖默认行为时，才需要这些设置：

```json
{
  "tomcat.path": "/absolute/path/to/tomcat",
  "tomcat.session.defaultCwd": "/absolute/path/to/workspace",
  "tomcat.serve.extraArgs": [],
  "tomcat.layout.controlsInset": 15,
  "tomcat.layout.contentInset": 25
}
```

侧栏间距在 **VS Code 设置** 中搜索 `tomcat.layout`，可写入用户或工作区设置；修改后执行 **Developer: Reload Window（重新加载窗口）** 生效。默认控件线 **15px**、内容线 **25px**，用户或工作区显式配置不会被覆盖。数值是距离侧栏真实边缘的像素，范围 0–40：`controlsInset` 控制会话栏和输入框边框，`contentInset` 控制回复、卡片、Todo、附件条。内容线应大于控件线（例如 15/25 或 15/27），框内文字才与回复对齐；否则框内缩进按 0 处理。正文从一行自然增长，最高为 webview 高度的 30%，超过后内部滚动；附件保持在框外。

按优先级从高到低：

- 如果你显式设置了 `tomcat.path`，它优先。
- bundled VSIX 包默认优先使用 bundled CLI。
- 纯扩展安装会回退到 `PATH` / shell discovery。

## 命令

这个扩展提供了这些命令：

- `Tomcat: Focus Agent Box`
- `Tomcat: Open Settings`
- `Tomcat: Restart Serve`
- `Tomcat: Start New Session`
- `Tomcat: List Sessions`

### 输入框资源命令

```text
输入 / → 选择命令（只插入文字）→ 发送执行 → 提示/错误气泡
```

- `/reload` 同时重扫 Skill 和插件工具，不必重启会话。外部 CLI 或手工改资源后主动执行；不是配置热加载，也不是 MCP 重连。
- `/install './带空格路径' agent` 与 `/uninstall 包名 agent` 自动同步资源。必须指定 `current-project`（`scope`）、`agent` 或 `global`；缺参数返回用法。
- 卸载用 `tomcat packages` 列出的包名，不是工具名。手工放入、不在账本的资源，需手动删除本层 `plugins/` 或 `skills/` 下目录，再 `/reload`。
- 菜单在开头、空格后或换行后触发，路径/URL 内不触发；句中隐藏 Terminal 组。只有开头的共享管理命令且纯文字才本地执行；附件、引用和未知文字仍发普通提示词。
- 等回包时禁用发送和 Build，不追加虚假用户消息。旧 Serve 没有命令表时保持原输入行为。

### 项目 Commands、Skills 与 Rules

```text
/ 菜单 → Skills / Commands / Terminal
Skills、Commands → 黄色调用标签 → 发送 → 后端保存正文快照
Terminal → 斜杠文字 → Tomcat 本地管理操作
```

项目 `.cursor/commands/**/*.md`、`.agents/commands/**/*.md` 出现在 Commands 组；frontmatter 的 `description` 可选，有描述才显示第二行。同名不同来源分别显示。每组默认 3 项加 **Show N more**，4 项时直接全显示。黄色调用标签没有 ×：点击选中或将光标移到相邻位置，用 Backspace/Delete 删除；历史气泡使用同一标签。历史 retry 使用发送时的快照，恢复到输入框再发则读当前文件。普通文件、选区引用可以重复添加。

两目录 `rules/` 下的 `.md`、`.mdc` 都只在布尔 `alwaysApply: true` 时生效，注入 **User Custom Instructions**，修改后下一用户轮次刷新；本期不支持 Cursor 的 globs、模型选择、规则专用手动触发。执行 `/reload` 可查看生效数量和跳过原因。运行时读取项目指令尊重 Deny、不弹工具路径确认，LLM 工具权限门禁保持不变。旧 CLI 仅显示 Terminal，恢复的调用标签会被拒绝发送而保留草稿。新版迁移旧草稿；旧运行时不能回放新 command/skill 类型的历史，需保留匹配版本。CLI 用法与详细边界见 [使用说明](../tomcat/docs/user-guide.md#project-commands-and-rules)。

## 故障排查

如果 Tomcat Agent Box 没有出现：

1. 在命令面板里运行 `Tomcat: Focus Agent Box`。
2. 如果右侧面板被隐藏了，先显示 `Secondary Side Bar`，然后再试一次。
3. 确认扩展已经安装并启用。
4. Reload VS Code 窗口。
5. 确认你的 VS Code 版本与扩展兼容。

如果 VS Code 提示 VSIX 不兼容：

1. 下载与你机器匹配的 platform-specific bundled VSIX。
2. 如果你的平台不在 bundled targets 之内，安装
   `tomcat-vscode-ext-0.1.3.vsix` 并自行提供 CLI。

如果扩展找不到 Tomcat：

1. 优先使用适合你平台的 bundled VSIX。
2. 否则，在终端里运行 `tomcat --version`。
3. 如果失败了，修复你的 `PATH` 或设置 `tomcat.path`。

如果已经找到了 Tomcat，但仍然无法初始化：

1. 点击 `Start Setup`。
2. 在集成终端里完成 `tomcat init`。
3. 如果 VS Code 没有自动重新连接，就点击 `I've Finished Setup`。

如果 Tomcat 在对话过程中退出：

1. 待回答的澄清问题会记录为 **Disconnected**，不会伪装成用户跳过。
2. 运行 `Tomcat: Restart Serve`。
3. 检查 `Tomcat` output channel 里的启动信息和 stderr 细节。

`ask_question.timeout_ms`、`TOMCAT_ASK_QUESTION_TIMEOUT_MS` 与
`TOMCAT__ASK_QUESTION__TIMEOUT_MS` 已删除。旧值仅触发迁移提示，不会创建问题截止时间，
也没有替代的 timeout 配置。

## Changelog

发布说明见 [CHANGELOG.md](CHANGELOG.md)。
