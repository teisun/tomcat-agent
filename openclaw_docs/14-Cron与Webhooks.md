# Cron 与 Webhooks

**设计思想**：Cron 由 Gateway server-cron 驱动，定时执行配置的 jobs；Webhooks 处理外部回调（如 Gmail Pub/Sub），与 hooks 配合。

---

## 一、Cron

- **cron/**：`openclaw/src/cron/`，types、store、schedule、normalize。
- **service/**：timer、jobs、ops、state，执行逻辑。
- **server-cron**：`openclaw/src/gateway/server-cron.ts`，Gateway 侧定时调度。
- **isolated-agent**：`openclaw/src/cron/isolated-agent/run.ts`，隔离运行 Agent 任务。
- **cron-cli**：添加、编辑、列表 cron jobs。

---

## 二、Webhooks

- **webhooks-cli**：`openclaw/src/cli/webhooks-cli.ts`，Gmail 等 webhook 配置。
- **gmail**：Gmail watch、Pub/Sub、Clawdbot hooks 集成。
- **gmail-ops**：Gmail 操作与 setup。
