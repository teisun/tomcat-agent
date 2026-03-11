---
name: /verify-openai-apis
id: verify-openai-apis
category: Workflow
description: 从 .env 读取 OPENAI_API_KEY 与 HTTPS_PROXY，列出并验证 OpenAI 接口（models / responses / chat.completions）
---

# OpenAI 接口验证（读取 .env）

本 command 用于在本地快速验证 OpenAI API 连通性与鉴权状态。  
默认读取 `pi-rust-wasm/.env` 中的 `OPENAI_API_KEY` 与 `HTTPS_PROXY`（以及可选 `HTTP_PROXY` / `ALL_PROXY`）。

---

## 1. 前置检查

1. 确认项目根目录为 `pi-rust-wasm/`。
2. 检查 `.env` 存在且包含：
   - `OPENAI_API_KEY=...`
   - `HTTPS_PROXY=...`（可空；若为空表示直连）
3. 若缺少 `OPENAI_API_KEY`，立即停止并提示用户补充。

---

## 2. 加载环境变量

执行：

```bash
set -a
source .env
set +a
```

然后回显（脱敏）：

- `OPENAI_API_KEY`：仅显示前 6 位与后 4 位
- `HTTPS_PROXY`：完整显示

---

## 3. 可验证接口（先展示让用户选择）

按下列编号展示并等待用户选择（支持多选，如 `1 2 3`）：

1. `GET /v1/models`：鉴权与网络连通性快速检查（推荐先跑）
2. `POST /v1/responses`：新一代统一生成接口
3. `POST /v1/chat/completions`：兼容聊天接口（历史项目常用）

若用户未指定模型，默认：

- `MODEL=gpt-5.2`

---

## 4. 执行命令模板

优先使用脚本一键执行（推荐）：

```bash
./scripts/verify-openai-apis.sh        # 交互选择
./scripts/verify-openai-apis.sh 1 2 3  # 非交互，直接验证 1/2/3
```

脚本路径：`scripts/verify-openai-apis.sh`

---

### 4.1 验证 1：`GET /v1/models`

```bash
curl -sS --fail-with-body \
  https://api.openai.com/v1/models \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "Content-Type: application/json"
```

### 4.2 验证 2：`POST /v1/responses`

```bash
MODEL="${OPENAI_MODEL:-gpt-5.2}"
curl -sS --fail-with-body \
  https://api.openai.com/v1/responses \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
    \"model\": \"${MODEL}\",
    \"input\": \"Reply with: pong\"
  }"
```

### 4.3 验证 3：`POST /v1/chat/completions`

```bash
MODEL="${OPENAI_MODEL:-gpt-5.2}"
curl -sS --fail-with-body \
  https://api.openai.com/v1/chat/completions \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
    \"model\": \"${MODEL}\",
    \"messages\": [{\"role\":\"user\",\"content\":\"Reply with: pong\"}]
  }"
```

---

## 5. 结果判定与输出格式

每个被选接口执行后，按以下格式输出摘要：

```text
[PASS] <endpoint> - HTTP 200
```

或

```text
[FAIL] <endpoint> - HTTP <status> / <error_message>
```

并附加排查建议：

- `401`：API Key 无效或未生效
- `403`：项目/组织无权限或模型无权限
- `407` / `CONNECT`：代理不可达或代理认证失败
- `429`：速率限制或配额不足
- `5xx`：上游服务异常，建议稍后重试

---

## 注意事项

- 不要把完整 `OPENAI_API_KEY` 打印到终端或日志中。
- 若 `HTTPS_PROXY` 指向 `127.0.0.1:7890`，需确保本地代理服务已启动。
- 本 command 只做接口验证，不修改仓库代码，不执行 git 提交。
