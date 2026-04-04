//! # VM Actor 模型
//!
//! 将 Wasm VM 封装在专属 `spawn_blocking` 线程中，
//! 外部通过 `VmActorHandle` 发送命令（Init/DispatchEvent/Shutdown），
//! 避免并发直接持有可变 Vm。

use crate::infra::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use super::WasmInstance;

/// VM actor 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VmActorState {
    Created = 0,
    Running = 1,
    Idle = 2,
    ShuttingDown = 3,
    Stopped = 4,
    Error = 5,
}

impl VmActorState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Created,
            1 => Self::Running,
            2 => Self::Idle,
            3 => Self::ShuttingDown,
            4 => Self::Stopped,
            5 => Self::Error,
            _ => Self::Error,
        }
    }
}

/// 宿主向 VM actor 发送的命令。
#[derive(Debug)]
pub enum VmCommand {
    /// 首次 session_start 时发送，让 _start 进入事件循环。
    Init,
    /// 宿主有事件要投递给插件。
    DispatchEvent {
        event_type: String,
        data: serde_json::Value,
        context: serde_json::Value,
    },
    /// session_end 或系统关闭。
    Shutdown,
}

/// 事件信封：从宿主投递给 JS 侧 waitForEvent 的数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: serde_json::Value,
    pub context: serde_json::Value,
}

/// 供外部（RuntimeManager / PluginManager）持有的 VM actor 句柄。
#[derive(Debug, Clone)]
pub struct VmActorHandle {
    pub cmd_tx: tokio::sync::mpsc::Sender<VmCommand>,
    pub state: Arc<AtomicU8>,
}

impl VmActorHandle {
    pub fn current_state(&self) -> VmActorState {
        VmActorState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// 向 actor 发送命令。
    pub async fn dispatch(&self, cmd: VmCommand) -> Result<(), AppError> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| AppError::Plugin("VM actor channel closed".into()))
    }

    /// 发送 Shutdown 命令。
    pub async fn shutdown(&self) -> Result<(), AppError> {
        self.dispatch(VmCommand::Shutdown).await
    }
}

/// VM actor：封装 WasmInstance，在专属线程中运行。
pub struct VmActor {
    instance: WasmInstance,
    script_path: PathBuf,
    cmd_rx: tokio::sync::mpsc::Receiver<VmCommand>,
    event_rx: std::sync::mpsc::Receiver<EventEnvelope>,
    state: Arc<AtomicU8>,
}

impl VmActor {
    /// 创建并启动 VM actor，返回句柄和事件发送端。
    ///
    /// `event_capacity`：有界事件 channel 容量（回压阈值）。
    pub fn spawn(
        instance: WasmInstance,
        script_path: PathBuf,
        event_capacity: usize,
    ) -> (VmActorHandle, std::sync::mpsc::SyncSender<EventEnvelope>) {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<VmCommand>(32);
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<EventEnvelope>(event_capacity);
        let state = Arc::new(AtomicU8::new(VmActorState::Created as u8));

        let handle = VmActorHandle {
            cmd_tx,
            state: state.clone(),
        };
        let event_tx_clone = event_tx.clone();

        let actor = VmActor {
            instance,
            script_path,
            cmd_rx,
            event_rx,
            state,
        };

        tokio::task::spawn_blocking(move || actor.run());

        (handle, event_tx_clone)
    }

    fn set_state(&self, s: VmActorState) {
        self.state.store(s as u8, Ordering::Relaxed);
    }

    fn run(mut self) {
        let pid = self.instance.plugin_id().to_string();
        // 1. Wait for Init command
        match self.cmd_rx.blocking_recv() {
            Some(VmCommand::Init) => {
                tracing::debug!("[VmActor {pid}] Init received");
            }
            Some(VmCommand::Shutdown) | None => {
                tracing::debug!("[VmActor {pid}] Shutdown/None before Init → Stopped");
                self.set_state(VmActorState::Stopped);
                return;
            }
            Some(_) => {
                tracing::warn!(
                    "[VmActor {}] received non-Init command before init, ignoring",
                    self.instance.plugin_id()
                );
                self.set_state(VmActorState::Error);
                return;
            }
        }

        self.set_state(VmActorState::Running);

        // 2. Build persistent Vm and run _start
        //    run_script(_start) blocks until JS event loop exits (details in 15.6)
        let run_vm_wall = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_vm()));
        let run_vm_wall_ms = run_vm_wall.elapsed().as_millis();
        tracing::debug!(
            "[VmActor {pid}] run_vm wall elapsed_ms={run_vm_wall_ms} (init_vm + _start; panic_caught={})",
            result.is_err()
        );

        // Init 之后 cmd_rx 在 _start 阻塞期间不再被轮询；Shutdown 等会积压在 channel 中。
        let mut drained = 0usize;
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            drained += 1;
            tracing::trace!(
                "[VmActor {pid}] discarding buffered cmd_rx after _start: {:?}",
                cmd
            );
        }
        if drained > 0 {
            tracing::warn!(
                "[VmActor {pid}] drained {drained} VmCommand(s) from cmd_rx after _start returned; \
                 these were not processed while VM blocked (Shutdown/DispatchEvent are no-ops on the actor thread during _start)"
            );
        }

        match result {
            Ok(Ok(())) => {
                self.set_state(VmActorState::Stopped);
                tracing::debug!("[VmActor {pid}] run finished state=Stopped");
            }
            Ok(Err(e)) => {
                tracing::error!("[VmActor {}] VM execution error: {e}", pid);
                self.set_state(VmActorState::Error);
                tracing::debug!("[VmActor {pid}] run finished state=Error");
            }
            Err(_panic) => {
                tracing::error!("[VmActor {}] VM thread panicked", pid);
                self.set_state(VmActorState::Error);
                tracing::debug!("[VmActor {pid}] run finished state=Error (panic)");
            }
        }
    }

    fn run_vm(&mut self) -> Result<(), AppError> {
        let pid = self.instance.plugin_id().to_string();
        tracing::debug!("[VmActor {pid}] run_vm: init_vm start");
        let (mut vm, _combined_path, _tmp_dir) = self.instance.init_vm(&self.script_path)?;
        tracing::debug!("[VmActor {pid}] run_vm: calling _start");
        let _start_t0 = Instant::now();
        let run_result = vm.run_func(Some("quickjs"), "_start", []);
        let _start_ms = _start_t0.elapsed().as_millis();
        tracing::debug!(
            "[VmActor {pid}] run_vm: _start returned ok={} elapsed_ms={_start_ms}",
            run_result.is_ok()
        );
        match run_result {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("exit code 0") || msg.contains("success") {
                    Ok(())
                } else {
                    Err(AppError::QuickJS(format!("_start failed: {msg}")))
                }
            }
        }
    }

    /// 获取事件接收端的引用（供 dispatcher waitForEvent 路由使用）。
    pub fn event_rx(&self) -> &std::sync::mpsc::Receiver<EventEnvelope> {
        &self.event_rx
    }
}

#[cfg(test)]
mod tests;
