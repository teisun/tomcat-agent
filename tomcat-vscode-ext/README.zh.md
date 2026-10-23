# Tomcat AI Agent for VS Code

<p align="center">
  <a href="https://github.com/teisun/tomcat-agent/blob/main/tomcat-vscode-ext/README.md">English</a> |
  <a href="https://github.com/teisun/tomcat-agent/blob/main/tomcat-vscode-ext/README.zh.md">简体中文</a>
</p>

Tomcat Agent Box 将 `tomcat serve --stdio` 运行时带到了专属的 VS Code 侧边栏中。

![Tomcat Agent Box screenshot](https://raw.githubusercontent.com/teisun/tomcat-agent/main/assets/tomcat-agent-box.png)

这个扩展仅适用于 **VS Code**。

## 从 VS Code 插件市场安装

需要 **VS Code 1.106.0 或更新版本**。在扩展面板搜索准确 ID **`tomcat-agent.tomcat-vscode-ext`**，确认发布者为 **tomcat-agent** 后安装；也可运行：

```bash
code --install-extension tomcat-agent.tomcat-vscode-ext
```

本次市场发布范围是 **Apple Silicon macOS（darwin-arm64）、Intel macOS（darwin-x64）、Linux x64（linux-x64）、Windows x64（win32-x64）**，每个平台包都内置对应 CLI。Linux 包使用 glibc。Windows 上 Agent 通过 Git Bash 执行命令，请先安装 [Git for Windows](https://git-scm.com/download/win)（`winget install --id Git.Git -e`）。Alpine/musl、Linux ARM64、Windows ARM64、远程开发及 vscode.dev 不在本次支持承诺内。原生发布验收记录实际 OS，不代表已经验证所有 Linux 发行版。

## 离线安装与 GitHub 下载

**0.1.90 发布渠道：** 本次更新仅发布到 GitHub，自动上传 Marketplace 仍关闭。请安装本版本 VSIX 获取新的首次配置流程，不要把 GitHub 发包当作市场已更新。

[GitHub Releases](https://github.com/teisun/tomcat-agent/releases) 保留平台 VSIX、仅 CLI 的压缩包，以及给已有 CLI 用户的纯扩展包。以对应 Release 的实际版本/文件为准，不照抄旧教程文件名。纯扩展包**不含 CLI**，不作为 Marketplace 通用回退包发布。

```bash
code --install-extension /absolute/path/to/downloaded-platform-package.vsix
```

Windows 上下载 `tomcat-vscode-ext-<版本>-win32-x64.vsix`，传入完整路径，例如在 PowerShell 中：`code --install-extension "$env:USERPROFILE\Downloads\tomcat-vscode-ext-<版本>-win32-x64.vsix"`。

安装后重新加载 VS Code。VS Code 会将安装包解压到扩展目录；内置 CLI 不是从 VSIX 文件或 Downloads 目录直接运行。

## 模型服务、费用与数据

通过 **Add Models...** 配置模型，并提供服务商要求的凭据。模型/API 服务可能单独收费。发送的消息、附件或选中上下文，以及相关工具结果可能发送到所配置的模型服务。文件改动和命令执行受 Tomcat 权限门控，请阅读授权请求再允许。这不代表所有处理都在本地，或所有模型服务都免费。反馈请使用 [GitHub Issues](https://github.com/teisun/tomcat-agent/issues)。

**内置 CLI 0.1.73 和 0.1.74 的已知限制：** 重载聊天历史后，被权限拒绝的写入可能错误显示 **Created（已创建）**，但文件实际上没有创建。权限拒绝仍然生效；请核对工具结果和实际文件，不要把恢复后的标签当作成功证明。此展示问题留待后续版本修复。

## 打开 Tomcat Agent Box

1. 按 `Cmd/Ctrl+Shift+P`。
2. 运行 `Tomcat: Focus Agent Box`。
3. VS Code 会展开 `Secondary Side Bar` 并聚焦到 Tomcat Agent Box。

你也可以自己打开 `Secondary Side Bar`，然后点击 Tomcat Agent Box 图标。

## 首次设置

1. 打开 Tomcat Agent Box，无需运行 `tomcat init`。
2. 面板提示「配置模型后即可开始对话」时，点击「配置模型」。
3. 点 **+ 连接供应商**，选一家供应商（中转选 **其它地址...**），填它的 API Key 并勾选要用的型号。第一个保存的型号自动成为默认模型，保存后即可对话，无需重启。一把 Key 一张卡片：同一把 Key 通用多种协议的中转只需连接一次，同一地址的第二把 Key 会单独成卡。详见[用户指南](https://github.com/teisun/tomcat-agent/blob/main/tomcat/docs/user-guide.zh.md#tomcat-model--不手改文件也能管模型与-key)。

会话里单独选过的模型、以及 `TOMCAT__LLM__DEFAULT_MODEL` 环境变量，优先于默认模型。

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

选择会话、置顶/取消置顶、显示更多、完成或取消删除后，下拉框仍保持展开。点击面板与触发器之外（包括 **+** 或齿轮）、按 Escape、或再点触发器才收起。删除确认打开期间，由确认框独占 Escape 与焦点，底层下拉框保持展开。

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

### 开始或继续计划

```text
明确要求实施 -> start_plan -> 如在 PLAN 模式，先确认
  -> 现有计划开工 -> 工作 / 提问 -> 回答 -> 继续
                               -> Stop -> pending
```

你要求实施或继续已保存的计划时，Agent 可以在同一回合直接开工。在 PLAN 模式内，现有问题面板先显示“开始执行 / 留在计划模式”，不提供自由输入；拒绝或跳过不会改变计划。其它模式下不额外确认。原有 **Build** 按钮与 `/plan build` 仍保留；普通 todo 编辑不会顺带开工。

执行中仍可提问，回答后继续当前回合；Stop 会结束回合并停放计划。

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
- `Tomcat: 重启共享服务（停止所有任务）`
- `Tomcat: Start New Session`
- `Tomcat: List Sessions`

### 输入框资源命令

```text
输入 / → 选择命令（只插入文字）→ 发送执行 → 提示/错误气泡
```

- `/reload` 同时重扫 Skill 和插件工具，不必重启会话。外部 CLI 或手工改资源后主动执行；不是配置热加载，也不是 MCP 重连。
- **主输入框**的 `/restart` 直接调用既有宿主重启命令，**停止共享服务的所有任务**。选择只补全文本，发送精确的纯文本命令才执行，不调用模型、不进入消息队列。带参数、引用、附件或同名资源卡时沿用普通消息路由。现有输入禁用条件不变；断连等无法发送时用命令面板。历史/队列编辑器不展示该入口，也不依赖服务端斜杠目录。
- `/install './带空格路径' agent` 与 `/uninstall 包名 agent` 自动同步资源。必须指定 `current-project`（`scope`）、`agent` 或 `global`；缺参数返回用法。
- 卸载用 `tomcat packages` 列出的包名，不是工具名。手工放入、不在账本的资源，需手动删除本层 `plugins/` 或 `skills/` 下目录，再 `/reload`。
- 菜单在开头、空格后或换行后触发，路径/URL 内不触发；句中隐藏 Terminal 组。只有开头的共享管理命令且纯文字才本地执行；附件、引用和未知文字仍发普通提示词。
- 等资源命令回包时禁用发送和 Build，不追加虚假用户消息。旧 Serve 没有资源命令表时保持原有资源命令路由；本地 `/restart` 入口不依赖该表。

### 项目 Commands、Skills 与 Rules

```text
/ 菜单 → Skills / Commands / Terminal
Skills、Commands → 黄色调用标签 → 发送 → 后端保存正文快照
Terminal → 斜杠文字 → Tomcat 本地管理操作
```

项目 `.cursor/commands/**/*.md`、`.agents/commands/**/*.md` 出现在 Commands 组；frontmatter 的 `description` 可选，有描述才显示第二行。同名不同来源分别显示。每组默认 3 项加 **Show N more**，4 项时直接全显示。黄色调用标签没有 ×：点击选中或将光标移到相邻位置，用 Backspace/Delete 删除；历史气泡使用同一标签。历史 retry 使用发送时的快照，恢复到输入框再发则读当前文件。普通文件、选区引用可以重复添加。

两目录 `rules/` 下的 `.md`、`.mdc` 都只在布尔 `alwaysApply: true` 时生效，注入 **User Custom Instructions**，修改后下一用户轮次刷新；本期不支持 Cursor 的 globs、模型选择、规则专用手动触发。执行 `/reload` 可查看生效数量和跳过原因。运行时读取项目指令尊重 Deny、不弹工具路径确认，LLM 工具权限门禁保持不变。旧 CLI 仅显示 Terminal，恢复的调用标签会被拒绝发送而保留草稿。新版迁移旧草稿；旧运行时不能回放新 command/skill 类型的历史，需保留匹配版本。CLI 用法与详细边界见 [使用说明](https://github.com/teisun/tomcat-agent/blob/main/tomcat/docs/user-guide.md#project-commands-and-rules)。

## 故障排查

如果 Tomcat Agent Box 没有出现：

1. 在命令面板里运行 `Tomcat: Focus Agent Box`。
2. 如果右侧面板被隐藏了，先显示 `Secondary Side Bar`，然后再试一次。
3. 确认扩展已经安装并启用。
4. Reload VS Code 窗口。
5. 确认你的 VS Code 版本与扩展兼容。

如果 VS Code 提示 VSIX 不兼容：

1. 下载与你机器匹配的 platform-specific bundled VSIX。
2. 其他平台不在本次支持承诺内；GitHub 纯扩展包需要另行提供可用且兼容的 CLI。

如果扩展找不到 Tomcat：

1. 优先使用适合你平台的 bundled VSIX。
2. 否则，在终端里运行 `tomcat --version`。
3. 如果失败了，修复你的 `PATH` 或设置 `tomcat.path`。

如果已经找到了 Tomcat，但仍然无法连接：

1. 看聊天面板里的失败原因，或点「查看日志」。
2. 修复后点「重试」。

如果 Windows 上执行命令时提示"在 Windows 上运行命令需要 Git for Windows（Git Bash）"：

1. 安装 Git for Windows：`winget install --id Git.Git -e`。
2. 重启 VS Code，让扩展拿到新的 `PATH`。不会使用 WSL 的 `bash.exe`。

`tomcat.config.toml` 中不支持的键会被忽略并在启动日志中告警，不再因此阻断 Serve 连接；已知键的非法值仍报错。旧连接信息不会自动迁移，需要时按现有模型配置引导添加。内置/纯包的 CLI 选择顺序以及显式 `tomcat.path` 的优先级不变。

如果 Tomcat 在对话过程中退出：

1. 待回答的澄清问题会记录为 **Disconnected**，不会伪装成用户跳过。
2. 运行 `Tomcat: Restart Serve`。
3. 检查 `Tomcat` output channel 里的启动信息和 stderr 细节。

`ask_question.timeout_ms`、`TOMCAT_ASK_QUESTION_TIMEOUT_MS` 与
`TOMCAT__ASK_QUESTION__TIMEOUT_MS` 已删除。旧值仅触发迁移提示，不会创建问题截止时间，
也没有替代的 timeout 配置。

## Changelog

发布说明见 [CHANGELOG.md](https://github.com/teisun/tomcat-agent/blob/main/tomcat-vscode-ext/CHANGELOG.md)。
