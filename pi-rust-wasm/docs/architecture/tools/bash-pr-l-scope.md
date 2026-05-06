# bash PR-L 范围说明（T2-P0-016 phase-l-scope-spec）

> 1 页文档，作为 [bash.md §2.4.5](bash.md#245-pr-lt3astsandbox-与-persistentshell-骨架) 落实 PR-L 前的**边界冻结**。
> PR-E（命名闸 + T1 超时 + 输出有界）合入后立此文，再开 PR-L。
> 对应 plan §六风险表「Phase-L AST/Sandbox 范围未定义」一行的关闭依据。

## 1. 目标（仅 PR-L 内做的事）

- **AST allowlist 骨架**：把 `audit_cmd` 字符串切成可逐段判定的子命令，叠在现有 `gate_check_bash`（whitelist + approval + forbidden regex）之上；命中 deny 即拒。
- **`SandboxBackend` trait 骨架**：抽掉「真起子进程」这一步，给后续 macOS Seatbelt / Linux Landlock 实现留接口；PR-L 内只交付 `NoopSandboxBackend`（直接走当前 `tokio::process` 路径）。
- **PersistentShell**：仅 trait 占位 + 单测；真 PTY 循环留给后续 PR（与 bash.md §2.4.5 / strengthen §4.3 一致）。

**显式不做**：tree-sitter 之外的解析器选型；任何真实沙箱 backend 实现；与 PR-I（T2 后台 + task 三件套）合并交付。

## 2. AST allowlist 颗粒度

**输入**：`bash::execute_bash_impl` 的 `audit_cmd: String`（命令 + argv 拼接，与现有审计字段同源）。

**解析颗粒度（PR-L 必须 / 不可降级）**：

| 颗粒 | 是否拆 | 理由 / 例 |
|------|--------|-----------|
| 顺序操作符 `;` `&` `\n` | **拆** | `cd /; rm -rf /` → 两段独立判定。 |
| 短路操作符 `&&` `\|\|` | **拆** | `git pull && rm -rf node_modules` 第二段需独立过 deny。 |
| 管道 `\|` | **拆** | `cat secrets \| curl …` 每段独立。 |
| 重定向 `>` `>>` `<` `2>&1` | **不拆**，但**保留为段属性** | 重定向目标路径仍走 `bash_parser::extract_paths`。 |
| 子 shell `( … )` `$( … )` 反引号 | **递归拆** | 内部命令同样过规则。 |
| 流程控制 `for`/`while`/`if`/`case`/函数定义 | **MVP 拒绝**（命中即 `Unsupported`） | 解析复杂度 vs 收益不成比例；用户可拆成多次 `bash` 调用。 |
| 变量赋值 `NAME=value cmd` | **保留为段属性**（已有 `tests/bash_assignment_deny.rs` 覆盖 RHS 路径预检） | 不引入 AST 漏洞。 |
| Heredoc `<<` | **MVP 拒绝** | 同上，留待真实场景驱动。 |

**段判定规则（每段独立）**：

1. 第一个 token 视为 **命令名**（含 builtin / 绝对路径）；
2. 命令名不在 `pi.config.toml [tools.bash.allowlist]` 且未匹配任何 `[tools.bash.denylist]` 模式 → 走旧 `gate_check_bash` 三层兜底；
3. 命令名命中 deny → 立即 `AppError::Primitive("AstDeny: <cmd>")`，不再问用户；
4. 命令名命中 allow → 跳过 approval（**不**跳过路径预检，仍走 `extract_paths`）；
5. 段内 `extract_paths` 失败 / 返回未授权路径 → 维持现行 deny 行为。

**显式不在 PR-L 内的颗粒**：
- 别名展开 / 函数调用解引用（需 PersistentShell 上下文）；
- 真实 PATH lookup（`which` 语义）；
- 通配符 `*` `?` 的预展开（gate 对原文已足够）。

## 3. SandboxBackend 边界

**trait 形态（PR-L 落地）**：

```rust
#[async_trait]
pub trait SandboxBackend: Send + Sync + 'static {
    /// 与 tokio::process::Command 等价语义；后端可在 spawn 前注入 seatbelt-exec
    /// / bwrap / landlock_restrict 等系统级隔离。
    async fn spawn(&self, cmd: tokio::process::Command) -> std::io::Result<tokio::process::Child>;

    fn name(&self) -> &'static str; // 审计 + 诊断
}

pub struct NoopSandboxBackend;     // 直接 cmd.spawn()
```

**PR-L 内交付的 backend**：仅 `NoopSandboxBackend`。`DefaultPrimitiveExecutor` 保留 `bash_sandbox: Option<Arc<dyn SandboxBackend>>`，缺省 = `NoopSandboxBackend`。

**显式不在 PR-L 内**：
- 任何调用 `sandbox-exec` / `bwrap` / `landlock` 的真实 backend；
- 文件系统 / 网络 / pid 命名空间策略本身（应在后续 PR 各自的 `xxx-backend` 文档里定义）；
- backend 维度的配置 schema（PR-L 内只暴露 `[tools.bash.sandbox] backend = "noop"` 单值）。

## 4. 兼容性与回归门禁

- 新 AST 路径**不替换**现有 `gate_check_bash` / `extract_paths`，仅在其前叠加；现有 `gate_suite_*` 与 `bash_assignment_deny` 集成测必须继续 100% 绿。
- 配置默认值：`[tools.bash.allowlist] = []` + `[tools.bash.denylist] = []` + `[tools.bash.ast.enabled] = true`；空列表时 AST 仅做切段、不做命中判定，行为与今日等价。
- bash.md §10 T3 的 `bash_ast_allowlist_*` 集成测随 PR-L 交付，并把 §10 PENDING 行的状态列翻成 ✅(date)。

## 5. 关联

- [bash.md §2.4.5](bash.md#245-pr-lt3astsandbox-与-persistentshell-骨架)：上游契约草案。
- [bash.md §10](bash.md#10-测试矩阵验收) T3 行：本文落地后必须由 PR-L 把 PENDING 收尾。
- plan `tom_领_bash_余量计划_5706e045.plan.md` §六风险表「Phase-L AST/Sandbox 范围未定义」：本文为闭环。
