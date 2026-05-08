# 集成/E2E 脚本「跑到一半卡住」整改说明

## 根因（最可能）

在 **Cursor / CI / 重定向输出** 等场景下，stdin/stdout **不是 TTY**。若环境里仍设置：

- `EDITOR` / `VISUAL` / `GIT_EDITOR` 指向 **vim**（或 vi 链到 vim）
- `PAGER` / `GIT_PAGER` 指向会等键盘的 pager

则 **cargo 或测试间接启动的子进程** 可能打开编辑器并 **阻塞等待输入**，表现为一动不动（「转圈」）。这与 Rust 测试逻辑无关，属于 **环境继承问题**。

次要噪音（易误判为失败）：

- `event_bus` 中 `listener_panic_is_caught_others_still_run` 会故意 `panic!`，stderr 仍打印 panic 行，但用例可被 `catch_unwind` 接住并通过。

## 修复

[scripts/run-integration-tests.sh](../../scripts/run-integration-tests.sh) 在 `cd` 到仓库根之后 **强制**（覆盖用户环境）：

- `EDITOR=true`、`VISUAL=true`、`GIT_EDITOR=true`（`true` 为立即退出的命令）
- `PAGER=cat`、`GIT_PAGER=cat`

并在 **release / lib / integration** 各阶段前后打印带时间戳的 `===` 行，便于对照日志判断卡在哪一步。

## E2E：`test_wasmedge_e2e_set_interval_runs_during_session` 失败（Stopped 而非 Running）

根因有两层：

1. **原 `pi_bridge` 用 `setTimeout(loop, 0)`**：`loop()` 在调度定时器后**立即返回**，`__pi_start_event_loop` 随之结束，同步脚本跑完；若 QuickJS `_start` 随之结束，VM actor 将 `run_vm` 视为正常结束并进入 `Stopped`。
2. **定时器需要 JS 执行机会**：长生命周期下需在两次宿主调用之间处理 `setInterval`；无限阻塞 `recv` 且无超时唤醒时，纯定时器脚本难以获得执行窗口。

修复：`waitForEvent` 支持 `params.timeoutMs`，超时返回 `{ type: "__tick" }`；`__pi_start_event_loop` 改为 `for (;;)` 轮询（默认 `timeoutMs: 50`），对 `__tick` 仅 `continue`，真实事件再走 `__pi_dispatch_event`。

## `end_session` 后测试长时间不结束（spawn_blocking 不退出）

根因：`cleanup_instance` 仅从 Dispatcher 移除 event channel，未主动通知 JS；且 `for(;;)` 退出后 QuickJS 仍可能处理 `setTimeout` 链，`_start` 不返回。

修复：

1. **`cleanup_instance`（[dispatcher.rs](../../src/ext/dispatcher.rs)）**：在移除 channel 前对实例 `try_send` 一条 `event_type: "__shutdown"` 的 `EventEnvelope`，让 `__pi_start_event_loop` 走既有 `__shutdown` 分支退出。
2. **`__pi_start_event_loop`（[pi_bridge.js](../../assets/js/pi_bridge.js)）**：在所有退出路径将 `setTimeout` / `setInterval` 置为空函数，避免 QuickJS 内部事件循环被无限自递归定时器拖住。

## 若仍卡住：手工排查

1. 单线程跑 lib，看最后一条 `Running` / `test ...`：
   `cargo test --lib -- --nocapture --test-threads=1`
2. 确认未在跑需真实网络的 **ignored** 用例（默认不跑）。
3. 对 integration：`cargo test --test cli_tests -- --nocapture --test-threads=1` 等逐个 crate 缩小范围。
