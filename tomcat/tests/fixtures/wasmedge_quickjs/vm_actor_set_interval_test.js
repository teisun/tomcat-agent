// Long-lived VM E2E: 会话级周期性 pi.log（E2E-WASM-033）。
// 使用 setTimeout 链（wasmedge_quickjs 对全局 setInterval 支持不稳定）。
var tickCount = 0;
function tick() {
  tickCount++;
  pi.log('vm_actor_set_interval: tick=' + tickCount);
  setTimeout(tick, 200);
}
setTimeout(tick, 200);

__pi_start_event_loop();
