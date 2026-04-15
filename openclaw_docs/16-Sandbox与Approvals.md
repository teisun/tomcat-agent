# Sandbox 与 Approvals

## 零、先用大白话

Sandbox 像 **游乐场护栏**：危险动作在 **圈起来的区域** 里做。  
Approvals 像 **家长签字**：要跑 shell、动大文件前，先 **点同意**。

**这一节你会学到**：白名单从哪读；审批走 Gateway 哪几个 method。

---

**设计思想**：Sandbox 基于 Docker 提供 Agent 隔离，策略由 tool-policy、sandbox 白名单控制；Approvals 管理 exec 等敏感操作的审批流程，Gateway 提供 exec-approval-manager 与 server-methods。

---

## ASCII 核心四图

### 1) 结构图

```text
Agent tool 请求
        |
        v
tool-policy / sandbox 白名单
        |
        v
敏感路径 -> exec-approval-manager
        |
        v
Docker 运行时（可选）
```

### 2) 调用流图

```text
toolCall 目标命令
  -> 判定是否需审批
      -> 等待 UI/CLI approve
          -> 在容器内执行或拒绝
              -> 结果回写会话
```

### 3) 时序图

```text
Agent        policy gate        Approval UI       Docker
  |              |                  |               |
  | exec         |                  |               |
  |------------->| 需审批           |               |
  |              |----------------->| 用户确认      |
  |              |-------------------------------->| run
```

### 4) 数据闭环图

```text
误拒/误批样本
        |
        v
收紧白名单或默认拒绝
        |
        v
审计日志复查
        |
        v
调整策略文件并热重载
```

---

## 一、Sandbox

- **`src/agents/sandbox/`**：docker、context、config、browser、tool-policy、runtime-status。  
- **`src/agents/sandbox.ts`**：`resolveSandboxContext`、创建参数。  
- **sandbox-cli**：recreate、explain、prune 等。  
- **sandbox-paths**：workspace 与容器路径映射。

---

## 二、Approvals

- **`src/gateway/exec-approval-manager.ts`**：exec 审批请求与响应的状态机。  
- **`src/gateway/server-methods/exec-approval.ts`**、**`exec-approvals.ts`**：Gateway Methods 对外 API。  
- **`src/infra/`** 一带：底层持久化/通知（以仓库搜索 `exec-approval` 为准）。

---

## 常见误会

- **误会**：开了 sandbox 就 100% 安全。**正解**：配置错、挂载目录错，仍可能 **数据泄露**；要纵深防御。  
- **误会**：点了 approve 以后永远放行。**正解**：有的实现按 **命令哈希/会话** 维度；读具体策略。  
- **误会**：主会话和非主会话 sandbox 一样。**正解**：**tool-policy** 对非 main 更严（常见默认）。
