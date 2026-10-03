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

## 可选设置

大多数用户 **不需要** 手动配置任何内容。

只有在你想覆盖默认行为时，才需要这些设置：

```json
{
  "tomcat.path": "/absolute/path/to/tomcat",
  "tomcat.session.defaultCwd": "/absolute/path/to/workspace",
  "tomcat.serve.extraArgs": [],
  "tomcat.layout.controlsInset": 10,
  "tomcat.layout.contentInset": 15
}
```

侧栏间距在 **VS Code 设置** 中搜索 `tomcat.layout`，可写入用户或工作区设置；修改后执行 **Developer: Reload Window（重新加载窗口）** 生效。数值是距离侧栏真实边缘的像素，范围 0–40：`controlsInset` 控制会话栏和输入框边框，`contentInset` 控制回复、卡片、Todo、附件条。内容线应大于控件线（例如 10/15 或 10/17），框内文字才与回复对齐；否则框内缩进按 0 处理。正文从一行自然增长，最高为 webview 高度的 30%，超过后内部滚动；附件保持在框外。

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
- 等回包时禁用发送、压缩和 Build，不追加虚假用户消息。旧 Serve 没有命令表时保持原输入行为。

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
