# 每会话执行沙箱与 Workspace 隔离

> 本文是 [`01-overview.md`](./01-overview.md) 的横切分册，覆盖 Phase B / C 的执行隔离、安全边界与 workspace 生命周期。
> 关联文档：[`../work-dir-and-data-layout.md`](../work-dir-and-data-layout.md)、[`../security.md`](../security.md)、[`../permission-system.md`](../permission-system.md)、[`../plan-runtime.md`](../plan-runtime.md)。
>
> 本文回答四件事：
>
> 1. **为什么云端 Tomcat 不能继续让 `bash/read/write/edit` 直接打到宿主共享文件系统？**
> 2. **每会话工作区该怎么挂、怎么快照、怎么回放、怎么跟 checkpoint 对齐？**
> 3. **网络出站、凭证、CPU/内存/磁盘等资源限制应该落在哪一层？**
> 4. **容器 / gVisor / Firecracker 怎么选，什么时候升档？**

---

## 文首导读：方案导图集

### 阅读顺序建议

1. 先看 **A.1 抽象图**：理解“会话 -> 沙箱租约 -> workspace overlay -> snapshot/blob”的主链路。
2. 再看 **A.2 具体图**：理解 Tomcat 当前 `primitive`、`checkpoint`、`net_guard`、`work_dir` 如何映射到新架构。
3. 再看 **B 生命周期**：理解沙箱如何从预热池进入绑定、暂停、销毁。
4. 最后看 **§3 决策矩阵**：这里会正式裁决默认选型与升级路径。

### A.1 抽象 ASCII 总图

```text
sessionKey
  │ request sandbox lease
  ▼
┌──────────────────────── Sandbox Provider ───────────────────────┐
│ allocate sandbox │ inject credentials │ apply resource limits    │
│ mount workspace  │ compile egress policy │ attach audit stream    │
└───────────────┬──────────────────────────────────────────────────┘
                │
                ▼
┌──────────────────────── Workspace View ──────────────────────────┐
│ base image / repo snapshot (read-only)                          │
│ + overlay writable layer (session-specific)                     │
│ + temp / logs / caches                                          │
└───────────────┬──────────────────────────────────────────────────┘
                │
                ├─ live shell / read / write / edit
                │
                └─ checkpoint / suspend
                    ▼
┌──────────────────────── Snapshot & Blob ─────────────────────────┐
│ workspace diff │ checkpoint metadata │ tool outputs │ audit refs │
└──────────────────────────────────────────────────────────────────┘
```

读图导读（说人话）：云端沙箱的核心不是“把进程塞进容器”这么简单，而是把 **执行环境、文件系统、网络、凭证、资源预算、恢复点** 统一到同一个租约里。对 Tomcat 来说，`bash/read/write/edit` 是最强能力，所以它们不可能继续共享一个宿主工作目录；否则只要某个工具、插件或 prompt 失控，就会跨会话污染文件和凭证。

### A.2 具体 ASCII 总图

```text
当前 Tomcat
────────────────────────────────────────────────────────────────────
src/core/tools/primitive/*
  ├─ read / write / edit
  └─ shell
       │
       ▼
宿主当前工作目录 / 授权目录
       │
       ├─ work_dir/agents/main/sessions/*
       ├─ work_dir/agents/main/tool-results/*
       └─ workspace-main / 外部 workspace roots

Checkpoint
  └─ src/core/checkpoint/shadow_git.rs

Network guard
  └─ src/infra/net_guard.rs  逻辑校验 URL / DNS / 私网

──────────────────────────── 目标 ────────────────────────────────

src/cloud/sandbox/provider.rs
  ├─ HostSandboxProvider       本地开发/Phase A fallback
  ├─ GvisorSandboxProvider     默认云端档
  └─ FirecrackerSandboxProvider 高隔离档

src/cloud/sandbox/workspace.rs
  ├─ prepare_base_snapshot(worktree/repo/template)
  ├─ mount_overlay(sessionKey)
  ├─ flush_overlay_diff()
  └─ restore_snapshot()

src/cloud/sandbox/egress_policy.rs
  ├─ compile from tenant/session/tool
  ├─ net_guard preflight
  └─ sandbox runtime enforcement

src/cloud/sandbox/credentials.rs
  ├─ short-lived env/secret mount
  └─ revoke on release

src/cloud/storage/blob_store.rs
  ├─ workspace diff
  ├─ tool-results
  └─ audit attachments
```

读图导读（说人话）：现在的 `net_guard` 更像“逻辑门卫”，只会在 HTTP fetch 之类路径上检查 URL 和 DNS；它并不能阻止 shell 命令、外部工具、第三方依赖或内核层绕开策略。目标架构里，`net_guard` 仍保留为第一道“策略编译与输入校验”，但真正的隔离要落在 sandbox runtime 和网络层。

### B. 状态机：SandboxLease 生命周期

```text
┌──────────┐ reserve ┌────────────┐ bind session ┌──────────┐ idle suspend ┌────────────┐
│ prewarmed│────────▶│ allocating │─────────────▶│ active   │──────────────▶│ suspended  │
└────┬─────┘         └────┬───────┘              └────┬─────┘               └────┬──────┘
     │ create new           │ policy/cred fail         │ release / ttl            │ resume
     ▼                      ▼                          ▼                          ▼
┌──────────┐          ┌────────────┐             ┌────────────┐             ┌──────────┐
│ creating │─────────▶│ degraded   │             │ destroying │────────────▶│ gone     │
└──────────┘          └────────────┘             └────────────┘             └──────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `prewarmed` | 分配给某会话 | `allocating` | 绑定 workspace overlay、凭证、egress policy | 从预热池里拿一个空工位开始布置。 |
| `allocating` | 所有前置成功 | `active` | 允许 shell/read/write/edit 真执行 | 工位布置好，可以开工了。 |
| `active` | 长时间空闲 | `suspended` | flush diff、撤销高权限凭证、保留最小恢复信息 | 人走了，工位先收起来。 |
| `suspended` | 会话恢复 | `active` | restore overlay、重新签发凭证 | 下次再来能接着干。 |
| 任意态 | 错误 / 策略失败 | `degraded` | 禁止高风险工具、打告警 | 工位有问题，别继续冒险。 |
| `active/suspended` | 会话关闭 / TTL 到期 | `destroying`→`gone` | 落最终快照、清理 secrets、销毁 runtime | 会话结束后，工位该彻底收走。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `SandboxProvider` | 统一的沙箱能力边界 | Rust trait + provider impl | API 稳定，后端可替换 | 先把“能力”定义出来，背后用什么实现再说。 |
| `SandboxLease` | 某会话占用某个沙箱实例的租约 | worker runtime + provider handle | 与 `sessionKey` 绑定；会话释放时必须撤销 | 这是“这个工位目前归谁用”的租约单。 |
| `BaseSnapshot` | 只读基础工作区快照 | blob/object ref + metadata | 对多个会话可复用 | 大部分人从同一个底板起步。 |
| `OverlayDiff` | 会话可写层产生的差异 | blob/object ref + manifest | 只属于单会话 | 每个会话自己的改动单独记。 |
| `EgressPolicy` | 出站网络允许列表与限制 | compiled policy + kernel/runtime enforcement | 既有逻辑校验，也有运行时硬限制 | 不只是“看 URL 像不像坏人”，而是“真的出不去”。 |
| `CredentialBundle` | 某会话在某沙箱里可见的短期凭证 | env vars / mounted files / tokens | 有 TTL、最小权限、可吊销 | 凭证像临时门卡，用完就失效。 |
| `Suspension` | 沙箱短期暂停并保留恢复点 | snapshot metadata + cache | 不等于永久保存；TTL 到期可销毁 | 人暂时走了，工位先封存。 |

## 2. 竞品 / 选型对比（调研）

### 2.1 执行隔离技术对比

| 方案 | 形态 | 优点 | 缺点 | 说人话 |
|------|------|------|------|--------|
| 共享宿主进程 + 逻辑权限 | 当前本地模型 | 最轻、接入成本最低 | 云端不安全、跨会话污染风险高、资源不可硬限制 | 本地能凑合，云端绝对不够。 |
| 普通 OCI 容器 | Linux namespace + cgroup | 成熟、性能好、镜像生态强 | 内核共享，默认隔离强度一般 | 够快，但安全边界还要额外加固。 |
| gVisor | 用户态内核沙箱 | 比普通容器强，集成 K8s 成本较低 | syscall 开销更高，兼容性需验证 | 默认云端档位的好平衡。 |
| Firecracker microVM | 轻量 VM | 隔离最强、租户边界清晰 | 启动/管理更重，镜像与调试复杂 | 高安全档位很香，但别一上来全量用。 |

### 2.2 威胁模型

至少覆盖这些威胁：

1. **跨会话文件污染**：A 会话改到 B 会话 workspace。
2. **跨租户凭证泄漏**：一个会话读到别家租户 API key。
3. **网络 SSRF / 内网探测**：通过 shell、curl、依赖安装等绕开逻辑层 URL 校验。
4. **资源耗尽攻击**：单会话拉满 CPU、内存、磁盘、inode、子进程数。
5. **长任务逃逸**：会话结束后后台 shell 仍继续跑，占资源或泄漏状态。

云端设计必须默认这些事情**会发生**，而不是默认 prompt 都是善意的。

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

本章先定默认 provider 和 workspace 模型，再定 egress、凭证、资源和生命周期策略。目标不是“理论最安全”，而是“默认够安全、可渐进升档”。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| S1 默认隔离实现 | 第一版云端默认用什么沙箱 | **采用** `SandboxProvider` 抽象，默认 `gVisor/容器池`，高安全租户可切 `Firecracker`；**拒绝**继续默认共享宿主执行。 | 本仓：`src/infra/net_guard.rs`、`primitive executor`、`work-dir-and-data-layout.md`；外部：`OpenClaw` deploy 经验、云端 agent 常见容器/VM 实践 | gVisor 在安全、接入成本、K8s 集成之间更平衡；Firecracker 作为升级档位更合适。 | 未入选：一上来全量 Firecracker；拒因：启动和运维成本高，容易拖慢整体落地。 | 先用更稳妥的隔离默认档，再给高风险场景上更硬的档位。 |
| S2 workspace 视图 | 工作区是 copy 还是 overlay | **采用** `read-only base snapshot + per-session writable overlay`；**拒绝**每会话全量拷贝 repo 或直接共享宿主目录。 | 本仓：`work-dir-and-data-layout.md`、`shadow_git.rs`；外部：容器 overlayfs 常规做法、OpenClaw 的持久卷经验 | overlay 兼顾空间效率和隔离，且最适合快照 diff。 | 未入选：全量 clone/copy；拒因：大仓库成本高、恢复慢。 | 同一个底板上叠个人改动，最省空间也最好回放。 |
| S3 快照与 checkpoint | workspace 恢复点怎么存 | **采用** `checkpoint metadata + overlay diff blob + optional git anchor`；**拒绝**只靠 transcript 或只靠 git。 | 本仓：`src/core/checkpoint/shadow_git.rs`、`src/core/session/*`；外部：`LangGraph` pending writes / durable checkpoint | transcript 只记对话，不记文件系统；git 只记文本版本，不记完整运行态。两者都不够。 | 未入选：只用 shadow git；拒因：对临时文件、工具缓存、二进制中间物支持差。 | 会话恢复点必须同时记住“说到哪儿”和“盘里长什么样”。 |
| S4 网络出站 | 只靠 `net_guard` 逻辑校验够不够 | **采用** `net_guard` 负责策略编译与 preflight，沙箱 runtime 负责硬 enforcement；**拒绝**只做应用层 URL 校验。 | 本仓：`src/infra/net_guard.rs`；外部：容器网络策略 / gVisor/firecracker 运行时隔离经验 | 逻辑校验能挡显眼错误，运行时限制才能挡绕路和未知路径。 | 未入选：只保留现有 `net_guard`；拒因：shell/curl/apt/pip 等路径很容易绕开。 | 门卫重要，但还得有门和围墙。 |
| S5 凭证模型 | API key / token 怎样进沙箱 | **采用** `CredentialBundle` 短期签发、最小权限、按会话挂载、释放即吊销；**拒绝**把长期凭证直接塞进共享 env。 | 本仓：`work-dir-and-data-layout.md` 的 credentials、`assets/.env` 现状；外部：多租户托管服务常规 secret mount 实践 | 凭证必须跟 `sessionKey`、tenant 和 TTL 绑定，才能在回收和审计时说得清。 | 未入选：进程级共享环境变量；拒因：泄漏面太大，回收困难。 | 凭证像一次性门卡，不该发成万能钥匙。 |
| S6 资源治理 | 限额在哪做 | **采用** cgroup/VM 级 CPU、内存、磁盘、inode、进程数、网络带宽限制 + 应用层 timeout；**拒绝**只靠应用代码超时。 | 本仓：`primitive` / `shell` 现状缺系统级隔离；外部：容器与 microVM 常规配额模型 | 应用层超时挡不住 fork bomb、内存爆炸和磁盘写满。 | 未入选：只靠 Rust 代码里超时；拒因：对子进程和内核资源不够硬。 | 真资源限制得落到系统层，不能只靠“请自觉”。 |
| S7 生命周期 | 工位用完是立即销毁还是可暂停 | **采用** `prewarm -> active -> suspended -> destroy` 生命周期；**拒绝**每次都从冷启动或长期常驻不回收。 | 本仓：Phase A/B HeatState 语义；外部：托管容器池和 serverless 预热池经验 | 预热池可压缩首 token 延迟，暂停态可减少重复启动成本。 | 未入选：每会话一次性全新启动、结束即销毁；拒因：交互型 coding agent 冷启动太痛。 | 工位要能预热、能短暂停、能最终回收。 |
| S8 本地开发兼容 | 本地调试怎么接入 | **采用** `HostSandboxProvider` 作为本地/CI fallback，但协议与生命周期保持同一 trait；**拒绝**本地和云端完全两套执行接口。 | 本仓：本地 `serve` 与 Phase A/B 设计；外部：Codex/其他 agent 的多 transport / 多 runtime 经验 | 本地调试和 CI 需要轻量 fallback，但不该破坏云端抽象边界。 | 未入选：本地继续直接调 primitive、云端另起接口；拒因：测试和生产语义会漂。 | 本地可以简化实现，但不能说另一种语言。 |

### 3.2 实施点

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| S-A Provider trait | `SandboxProvider`、`SandboxLease`、本地/gVisor/Firecracker 实现骨架 | `src/cloud/sandbox/provider.rs` | 见本文 §8.1 | 先统一接口。 |
| S-B Workspace overlay | base snapshot、overlay mount、diff flush、restore | `src/cloud/sandbox/workspace.rs`、`blob_store.rs` | 见本文 §8.2 | 让每会话都在自己的工位上改。 |
| S-C Egress policy | 策略编译、`net_guard` 扩展、runtime enforcement hook | `src/cloud/sandbox/egress_policy.rs`、`src/infra/net_guard.rs` | 见本文 §8.1 / §8.3 | 把“能访问什么网”钉死。 |
| S-D Credentials & limits | 凭证签发/吊销、cgroup/VM limit、审计流 | `src/cloud/sandbox/credentials.rs`、`limits.rs` | 见本文 §8.2 / §8.3 | 工位既要有门卡，也要有电表。 |
| S-E Pool & lifecycle | prewarm pool、suspend/destroy、TTL、回收器 | `src/cloud/sandbox/pool.rs`、`lifecycle.rs` | 见本文 §8.4 | 工位池得会周转。 |

#### 3.2.1 S-A：Provider 抽象

推荐 trait 形状：

```text
SandboxProvider
  - reserve(spec) -> SandboxLease
  - restore(snapshot_ref, spec) -> SandboxLease
  - suspend(lease) -> SuspensionRef
  - destroy(lease)
  - health()
```

其中 `SandboxLease` 至少包含：

- `leaseId`
- `sessionKey`
- `providerKind`
- `workspaceRoot`
- `networkHandle`
- `credentialHandle`
- `limitsHandle`

#### 3.2.2 S-B：Workspace 模型

推荐工作区布局：

- `BaseSnapshot`
  - 代码模板 / repo 初始镜像 / 共享缓存
- `WritableOverlay`
  - 当前会话真实修改
- `EphemeralScratch`
  - 临时文件、日志、下载缓存

与现有 checkpoint 的关系：

- 文本源码状态仍可用 git anchor 表达
- 非 git 资产（临时文件、二进制中间物、依赖缓存）走 overlay diff
- checkpoint metadata 记录两者引用

#### 3.2.3 S-C：Egress 政策链

推荐三层：

1. **策略编译层**：tenant + tool + workspace policy -> `EgressPolicy`
2. **逻辑 preflight**：扩展 `net_guard` 做 URL/DNS/私网校验与告警
3. **运行时 enforcement**：在 sandbox 网络命名空间 / VM 层限制实际出站

这样即使 shell 里直接 `curl`，也不会因为没走 Rust helper 就绕过。

#### 3.2.4 S-D：凭证与限额

凭证建议：

- 短期 token
- 每会话挂载目录或 env
- 可审计 issuance/revoke
- 按工具与 tenant 最小权限分发

限额建议：

- CPU shares / quota
- memory hard limit
- disk bytes + inode
- process/thread count
- wall clock timeout
- network egress bytes

#### 3.2.5 S-E：预热池与暂停

预热池的意义：

- 首 token 延迟更低
- 避免每次都从 0 启 sandbox

暂停态的意义：

- 审批等待、短时 idle、短断线重连时，不必立刻销毁

销毁条件：

- session close
- tenant quota 回收
- TTL 到期
- worker drain / quarantine

## 4. 协议

### 4.1 `SandboxLeaseSpec`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKey` | `string` | 是 | - | reserve/restore | 会话主键 | 这工位归谁。 |
| `providerClass` | `string` | 是 | - | reserve/restore | `host/gvisor/firecracker` | 用哪档隔离。 |
| `workspaceTemplateRef` | `string` | 是 | - | reserve/restore | base snapshot 引用 | 底板从哪来。 |
| `credentialProfile` | `string` | 否 | tenant default | reserve/restore | 凭证模板 | 要挂什么门卡。 |
| `limits` | `object` | 是 | - | reserve/restore | CPU / memory / disk / proc 等限制 | 工位配多大电表。 |
| `egressPolicyRef` | `string` | 是 | - | reserve/restore | 网络策略引用 | 能出哪些网。 |

### 4.2 `WorkspaceSnapshotRef`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `baseSnapshot` | `string` | 是 | - | suspend/restore | 基础快照 ID | 公共底板。 |
| `overlayDiff` | `string` | 是 | - | suspend/restore | 差异 blob ID | 你的改动层。 |
| `checkpointId` | `string` | 否 | - | checkpoint 对齐 | 关联 checkpoint | 对应哪个恢复点。 |
| `toolCacheRefs` | `string[]` | 否 | `[]` | suspend/restore | 可复用缓存 | 某些缓存可以带着走。 |

### 4.3 `EgressPolicy`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `mode` | `string` | 是 | - | network | `deny_all/allow_list/managed` | 默认全禁还是白名单。 |
| `allowedDomains` | `string[]` | 否 | `[]` | network | 域名白名单 | 能访问哪些站。 |
| `allowLocalNetwork` | `bool` | 否 | `false` | network | 是否允许私网 | 内网一律默认禁。 |
| `maxBytesPerRun` | `u64` | 否 | tenant default | network | 单 run 出站字节配额 | 别无限下东西。 |

## 5. 文件职责总览（One-Glance Map）

| 文件 / 模块 | 职责 | 说人话 |
|-------------|------|--------|
| `src/cloud/sandbox/provider.rs` | 定义 provider trait 和 lease 生命周期 | 沙箱世界的总接口。 |
| `src/cloud/sandbox/workspace.rs` | overlay、snapshot、restore、cleanup | 工位的桌面和抽屉。 |
| `src/cloud/sandbox/egress_policy.rs` | 编译网络策略并对接 runtime enforcement | 能上哪些网的规则表。 |
| `src/cloud/sandbox/credentials.rs` | 凭证签发、挂载、吊销、审计 | 门卡管理。 |
| `src/cloud/sandbox/pool.rs` | 预热池和回收器 | 工位池管理员。 |
| `src/infra/net_guard.rs` | Phase C 变成策略预检与解释层 | 逻辑门卫，负责看门但不独自扛墙。 |

## 6. 配置与环境变量

| 配置项 | 默认建议 | 说明 | 说人话 |
|--------|----------|------|--------|
| `cloud.sandbox.default_provider` | `gvisor` | 默认云端隔离档位 | 先用稳妥默认。 |
| `cloud.sandbox.hardened_provider` | `firecracker` | 高风险租户升档 | 更硬的安全套餐。 |
| `cloud.sandbox.prewarm_pool_size` | 按负载模型 | 预热池规模 | 提前备多少空工位。 |
| `cloud.sandbox.suspend_ttl_ms` | `300000` | 暂停态最长保留时长 | 工位空多久先封存。 |
| `cloud.sandbox.workspace.max_diff_bytes` | tenant/workload default | overlay diff 上限 | 改动太大就得额外治理。 |
| `cloud.sandbox.allow_local_network` | `false` | 是否允许访问私网 | 内网默认一律禁。 |

## 7. 错误模型 / 截断 / 警告

| 类别 | 条件 | 对外表现 | 恢复策略 | 说人话 |
|------|------|----------|----------|--------|
| `sandbox_allocation_failed` | provider 无可用实例或启动失败 | turn 延迟或失败 | 尝试其他 provider class / 排队等待 | 工位没抢到。 |
| `egress_denied` | 访问域名/地址不在策略内 | 工具返回安全错误 + 审计记录 | 提示用户申请额外权限 | 这网不能出。 |
| `credential_expired` | 会话恢复时旧 token 失效 | 重新签发或进入 degraded | 控制面重发 bundle | 门卡过期了。 |
| `overlay_limit_exceeded` | workspace diff 超上限 | 会话转只读或要求 checkpoint/compact | 清理缓存或人工扩配 | 改动太多，工位快塞满了。 |

## 8. 测试矩阵（验收）

### 8.1 单元测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| `EgressPolicy` 编译 | 私网/单段 host/非法协议被拒 | 规则表不能放水。 |
| `CredentialBundle` 生命周期 | 过期、吊销、重复释放都正确 | 门卡发放与回收有纪律。 |
| `WorkspaceSnapshotRef` 序列化 | base/diff/checkpoint 引用稳定 | 恢复点描述得完整。 |

### 8.2 集成测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 两个会话同时修改同一路径 | 互不污染 | 各改各的文件。 |
| suspend -> resume | overlay diff 恢复后文件系统一致 | 暂停后还能接着干。 |
| checkpoint + workspace snapshot | 文本改动和非 git 文件都能恢复 | 光有 git 还不够。 |

### 8.3 E2E / 安全测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| shell 尝试访问私网地址 | 被 runtime 拒绝，不依赖应用层 helper | 不能靠走偏门绕过去。 |
| 会话结束后后台进程存活检测 | 进程被清理，凭证被吊销 | 人走电断门锁。 |
| 不同 tenant 会话凭证隔离 | 永远看不到别家 secret | 这个是底线。 |

### 8.4 负载与容量测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 预热池耗尽 | 队列和降级路径可控 | 工位不够时也别乱。 |
| 大仓库 overlay diff | snapshot/flush 延迟仍在预算内 | 大项目也得扛住。 |

## 9. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| gVisor 兼容性不足 | 某些工具或依赖异常 | provider abstraction + per-tenant/provider override | 默认档位不行时要能切。 |
| Firecracker 成本过高 | 启动慢、调试难、运维复杂 | 作为升档选项，不做全量默认 | 最硬的不一定该一开始全员发。 |
| 只做逻辑 `net_guard` 不做 runtime enforcement | shell/工具绕过网络限制 | 必须把 enforcement 下沉到 sandbox 网络层 | 门卫不能只靠嘴。 |
| overlay diff 无限增长 | 存储和恢复成本飙升 | diff 上限、checkpoint/compact、缓存目录剔除 | 工位垃圾要定期清。 |

## 10. 历史决策 / 跨文档修订

1. 本文把 `net_guard` 从“工具层 URL 校验器”提升为“沙箱网络策略编译器的一部分”，但不把它误当成完整隔离实现。
2. 本文默认 `gVisor` 为云端常规档位，`Firecracker` 为高隔离档位；若团队后续有充分运维能力，可把默认档位提升，但不应反向破坏 `SandboxProvider` 抽象。
3. Workspace snapshot 与 checkpoint 的结合点，会直接影响 [`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md) 的 failover 成本与恢复语义。
