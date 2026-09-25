# MCP 并发整改：rmcp 3.4.0 的 M0 实测证据

基线：`1b1f35bc`。本记录区分 SDK 能力、Tomcat 适配与最终产品验收；不把直接 Peer 用例当作 manager/双会话/安装态已经通过。

```text
同一个受控 HTTP 服务 / 同一份客户端
  A 被闸门挡住 ── B 先完成                 → 证明并发，不靠耗时猜测
  取消 A/B ───── C 继续完成                 → 证明取消隔离
  客户端 POST/SSE Drop 探针归零             → 证明 HTTP 资源释放
  SDK 私有 pending/progress 表、真实会话链路 → 仍需 M1/M2 补证
```

## 升级与兼容

- `tomcat/Cargo.toml` 精确约束 `rmcp = "=3.4.0"`，锁文件也是 3.4.0。
- 锁文件仅两项版本/checksum 变化：rmcp 3.1.4 → 3.4.0、process-wrap 9.1.0 → 10.0.0；没有额外新增依赖包。
- 默认与 `test-streamable-http-server` 两种依赖树均保持 reqwest native-tls；server feature 仍由测试 feature 启用，未打开 SDK 默认 feature。
- 本机 rustc 1.96.0；两版 SDK 声明 MSRV 1.88。这不是用 1.88 编译的证据。
- 升级编译发现测试服务器使用已弃用的 `ServerInfo` 名称；已改为同一类型的新名 `ServerConfig`。

升级批次 `task:1790050945595-ie6w5j`，退出码0：计划批次A的 cargo update、默认/测试 feature tree、process-wrap反查、`cargo check --locked --lib --bins --features test-streamable-http-server`，以及原有 connector 单元53、HTTP集成2、stdio集成4全部通过。后续夹具改动由下列新批次覆盖。

## 共享夹具与能力验证

- `tomcat/tests/support/test_streamable_http_server.rs` 沿用同一个二进制，原 OAuth 模式不变；显式 `--faults` 开启测试控制路由。
- `tomcat/tests/support/streamable_http_faults.rs` 提供 HTTP 请求闸门、延迟头/部分JSON/长SSE、匹配及错误token进度、心跳、截断、会话过期、握手/取消挂起、逐请求执行/取消与响应退出观测；支持 legacy initialize 和 modern discover。
- `tomcat/tests/support/mcp_http_probe.rs` 包装真实 reqwest 适配，仅用于测试。HTTP工具请求开始时持有计数 guard，SSE打开后将 guard 转移到实际客户端响应流；真实 Drop 才减计数，不用服务端 socket 数冒充本地释放。
- `tomcat/tests/fixtures/mcp/fake_stdio_server.mjs` 持续读输入、逐请求处理，新增文件闸门、进度、逐请求取消与事件记录；保留原模式和图片回流结果。

`task:1790053075433-ex16mh`，退出码0：stdio集成6项（新增16笔同时到达、取消其中一笔且另一笔能完成；unit handler进度续期）和受夹具影响的manager单元16项通过。

`task:1790053535164-2cdp04` 的第一条测试命令通过：HTTP集成9项通过、1个原生SDK反例探针显式忽略。覆盖三种响应阶段、同/不同工具B先完成、请求ID/会话归属、无重放、流级错误不等于共享channel关闭、HTTP新旧生命周期下的进度续期、取消适配后的IO释放与C继续完成。

## 已复现的 SDK 残余缺口与最小适配

同一批次的第二条命令明确执行该反例（整体任务退出码101，不能称整批通过）：

```sh
cargo test --manifest-path tomcat/Cargo.toml --locked --features test-streamable-http-server --test connector_http_tests sdk_m0_saturated_legacy_cancellation_meets_five_second_cleanup -- --ignored --exact --nocapture
```

观测输出：`raw SDK after K: cancellation finished=false, retained tool HTTP/SSE owners=1`。两个legacy SSE调用的取消控制POST串行，前一个挂起；五秒后第二个取消仍未结束，且客户端确实保留一个工具HTTP/SSE资源。探针释放闸门并回收任务/子进程后才断言失败。

源码原因：SDK `RequestHandle::cancel` 的等待者与service独立拥有的transport发送任务不是同一个future；worker取消send future的DropGuard才释放请求lifetime。SDK默认control_request_timeout仅包含出队后的HTTP阶段，不含控制队列等待；legacy SSE读停止后仍等待lifetime结束。

最小适配原型在 `tomcat/src/core/connector/mcp/transport.rs::CancellationBoundedTransport`：仅对取消通知的**底层send future**设置包含排队的1秒投递期限，超时实际drop该future并返回明确投递错误，保留K=5秒中的剩余时间供本地收束。不限制普通请求总时长，不关闭共享连接，不重写进度路由，不修改registry源码或vendor SDK。

`bounded_transport_reclaims_cancelled_io_without_closing_other_requests` 已在真实HTTP的legacy/modern × 三种响应阶段通过：取消A/B后客户端仅剩C的资源，C在原客户端完成且只有一次握手。原生SDK反例保留为显式探针，不纳入“全部绿色”的宣称。

**M0阶段边界：** 当时适配仅被能力测试接入，不能将该批结果当作生产调用路径已交付。后续M1进展如下；SDK私有表计数、提交取消、完整退役及真实入口仍不能仅由HTTP资源Drop推断通过。

## M1 初步接入与定向验证（尚非最终验收）

生产已接入 `CancellationBoundedTransport`、`CallRuntime` 与 `ScopedHttpClient`：manager以N个执行名额和2N个准入名额替换完整调用锁；Q覆盖等待/提交，SDK独立管理本请求可续期I；调用者drop只取消child token，受跟踪任务继续驱动`await_response`收尾。HTTP响应所有权通过SDK不序列化的Extensions携带，结果和IO收尾后才放名额；没有第二份wire ID/进度路由表。

- `task:1790054954017-m2cxq5`，退出码0：新增生产模块的`cargo check --locked --lib --features test-streamable-http-server`通过。
- `task:1790055647030-69qdub`，退出码0：manager HTTP集成5项通过。缺省16及显式1/32 × 延迟头/JSON/SSE验证16+16准入、立即Busy、H=N、结果归属与一次握手；混合10次drop/Stop/超时后任务和额度回基线，B持续成功；业务错误/断流保留连接；进度不续期准入队列；**真实35秒SSE答案成功**。这不是“每一种终态各重复10次”的完整T1.2证据，也不是两个真实Serve会话。
- `task:1790056328762-i7pzif`，退出码0：connector单元56项、stdio集成8项通过。不过stdio夹具仍在写事件文件时临时目录被删除，输出ENOENT；不隐去这项测试清理竞态。
- 夹具改为先刷完completed/exited记录再发最终回答，并对带日志的stdio用例等待所有exited事件；`task:1790057057247-a2mdkb`，退出码0：Node语法检查和stdio集成8项通过，ENOENT不再出现。覆盖manager缺省16笔同时在途/第17笔等待、显式1、取消一笔后排队B完成且其他A仍等待，以及匹配进度续期/错token兄弟超时。

已知实现边界继续保留：SDK的`post_request`也观察`send_request.responder.closed()`（3.4.0 `streamable_http_client.rs:635-652`），所以取消底层send不仅影响等待者；正常结果先移除请求流注册再送handler（`:1742-1757`）。这些源码支撑现有薄适配，不能单靠阅读替代SDK所有私有表或合法GET续读的退出测试。

**待完成：** manager新旧协议组合、每类终态/提交取消/退役的10次回收与watcher证明、120/110/230时序、授权/启动及诊断脱敏、两条工具入口和VM取消、统一恢复/权限竞争、真实双会话及安装态。当前`CallRuntime`清理超出K只记录不变量失败并保留名额，不可据此宣称任意异常路径都已证明K内退出；未补证项目保持计划未完成。
