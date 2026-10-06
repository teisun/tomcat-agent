# 最近改动轮的 Files 面板

```text
第1轮改文件 -> 第2/3轮继续问答 -> 第4轮第一次实际修改
  Files=u1         仍是u1                Files整份切换=u4
                                           |
               u4改前备份 <-> 当前磁盘 -> +N -M
                          <-> 真实编辑器 -> 完整文件diff
                          <-- 确认恢复   -> 文件回滚，聊天不变
```

## 体验与范围

- 方案B：聊天记录 → 待回答区 → Todos／Files → Composer；外框四角4px，两组之间4px，边缘对齐。底部容器及输入框稳定挂载；附件条属于既有输入区域，不被当成另一个dock。
- Files只展示最近一个有实际原生文件修改记录的有效Normal用户轮次；问答/只读/失败/拒绝/取消/no-op不换轮。新轮首次成功改变字节或存在性才整表替换，不合并旧轮路径。
- 同轮同路径采用该轮第一次实际修改前的原件；反复修改合并成净比较。恢复或改回原样后文件行消失，但来源轮记录不删除，不回退复活更早清单。
- 这是备份对磁盘的视图，不是AI字符级归因或Git状态。来源轮已知路径内后续保存的人工/终端/其他会话修改也会进入比较；未保存编辑可在真实diff右侧出现，行数保存后刷新。
- 只覆盖原生write/edit/hashline_edit的备份路径；不承诺shell/MCP/插件单独修改的覆盖。Build/Signal/Nudge沿现有归属规则记到最近Normal用户消息，不新增owner或压缩机制。

## 架构与接口

Rust `core/checkpoint/session_files.rs` 按既有备份清单的最后修改时间选最近改动轮。来源选择只遍历安全的普通轮次目录，对 `baselines.jsonl` 做 `symlink_metadata`：不存在、非普通文件或0字节清单不参与；以 `(清单修改时间, 目录名)` 最大值稳定决出来源，不按目录名推测消息先后，不解析其他轮清单，也不扫描 transcript 正文。仅选中清单由既有 `read_rows` 严格解析；旧清单损坏不会影响当前来源，最新清单损坏则报错，不回退旧轮。`list/restore` 仍读 transcript 首行 cwd 做 HEAD 检查。作废轮在历史作废路径中清理备份目录；没有新来源指针、序号或存储格式。扩展 `SessionRouter` 验证响应，provider按会话缓存摘要并合并刷新。GUI `SessionDock` 只拥有展开/标签；`SessionFilesList` 拥有确认与请求提示。

| 命令 | 作用 |
|---|---|
| `get_session_files(sessionId)` | 返回sourceTurnId或null及完整单轮摘要 |
| `get_session_file_baseline(sessionId,sourceTurnId,path)` | 返回明确轮次的完整原件，原本不存在则空文本 |
| `restore_session_files(sessionId,sourceTurnId,paths)` | 整体预检确切路径后逐文件调用既有RestoreFiles |

能力`session_files`可选；旧CLI不查询、不显示Files。路径只能在服务端对应会话/轮manifest内解析，不接受客户端提供的backup路径或字节。两条动作命令不重新扫描历史判断当前来源，不静默换用其他轮备份。

刷新沿用checkpoints入口，加原生工具结束、当前 Files 目标文件保存、窗口重新聚焦、展开和恢复完成；保存匹配复用dirty保护的realpath身份与path.resolve fallback，仅检查活动会话缓存。不按token查，不做FileSystemWatcher，不强制加载聊天旧页。目标曾改回原样而暂不在清单时，之后手动保存不会触发刷新，聚焦/展开等其他既有入口会重新比较来源轮路径。失败保留最后成功数据并标错误；整表换轮、同源空表都要接受。sourceTurnId也进入虚拟文档URI，防止同名文件跨轮原件串用。

## Diff 与恢复

Files调用一次`vscode.diff`：左侧完整只读原件，右侧真实`file:`可编辑。不是“片段1/8”多条虚拟文档；同轮多次修改以同一before合并。旧工具卡`openDiffPreview`完整/片段两条行为都保持。二进制或超文本预算不伪造完整预览。

Files diff与恢复的已知错误由host共用 `sessionFileErrorText` 转成可读英文，覆盖binary、too_large、原件缺失、轮记录不可用、HEAD变化、busy和带路径原因的恢复失败；其他错误沿用桥接错误提示，不改wire错误码。通知不等待关闭，失败后继续刷新，恢复结果事件使用相同的可读detail。

×和Undo All先英文确认，说明后续已保存修改会覆盖、新建文件会删除、聊天保留。Undo All只发可恢复子集并说明跳过数。前台AI/维护忙时禁止恢复，但可看diff；后台Bash、其他会话/worktree、外部写入不检查、不停止。确认后内容改变也按用户选择直接覆盖，不加hash冲突校验、prepare票据或二次提醒。Host仅检查此次目标的dirty buffer，不自动save/revert。

保留HEAD闸和符号链接/特殊文件防护；普通文件原子写回并恢复备份权限，新建文件精确remove_file、不删父目录。多文件不是事务：预检失败不写盘，中途IO失败报告路径，刷新反映已恢复与剩余文件。Files不改transcript/队列/草稿，不请求新模型轮次。

## 生命周期

保持现有整轮prune（默认7天、Serve启动housekeeping）与作废轮次discard。新Files不是全会话汇总，因此不保留作废轮来源，不新增“删内容留清单”机制或过期数量。原件意外缺失但manifest在时，行保留为`backup_missing`、增删数未知、不可恢复；不拿其他轮原件补位。新建文件在记录存在且其他检查通过时可确认删除。

## 参考与推翻条件

- Cline `SdkController.computeLatestCheckpointChanges`以最新request checkpoint对当前工作区（本地`apps/vscode/src/sdk/SdkController.ts:1755`）；空轮会隐藏，Tomcat按用户需求不照搬。
- OpenCode `turnDiffs/reviewDiffs`可区分Git/Branch/Last turn（本地`packages/app/src/pages/session.tsx:652`）；Review与历史Revert不可混用。
- VS Code普通Chat `renderChatEditingSessionState`是跨轮尚未处理编辑；`ChatEditingSession.reject`按URI操作，不改变历史边界。Tomcat不复制其Accept与实时rebase。

来源按mtime的前提是：原生写入只归属最近Normal轮；较早轮清单不会在新轮之后继续追加；作废路径调用整轮清理。真实Retry、编辑旧消息和checkpoint式作废回归验证清理成功时的目录删除与来源重选，不代替删除失败或进程崩溃证明。既有 `discard_turns` 是best-effort（忽略IO删除错误），历史提交与目录清理也不是跨文件事务；删除失败或提交后清理前崩溃仍可能残留作废目录，按本方案可能被选中，此限制未由本次整改解决。

以下任一变化需要推翻mtime作为顺序代理，重新评估清单行中的单调序号及必要的失效处理：
- 迁移、导入、复制或恢复备份目录不保留修改时间，或外部touch/时钟回拨改变时间顺序。
- 写入可以继续归属较早轮，或Build独立成轮改变现有归属规则。
- 新增历史作废路径未清理备份目录，或要求在清理失败/崩溃边界下也严格排除作废轮。

用户要求多轮审阅、AI-only撤销、Redo/整组事务或非原生写入覆盖时再设计对应机制；性能不足时才添加可重建索引。不得因推测并发风险恢复已经撤掉的全局锁/票据。

## 证据分工

- Rust聚焦tests：来源、字节、路径与恢复；真实Serve integration：跨问答/再次修改/恢复且聊天字节不变。
- VS Code devhost：实际文件行点击、完整原生diff、可编辑右侧与dirty保护；fake Serve不能替代Rust字节证据。
- `scripts/accept-session-files-ui.mjs`：真实App/CSS、宽窄短视口、鼠标hover与几何，产物包括PNG/ARIA/console/geometry；模拟host不冒充真实后端。
