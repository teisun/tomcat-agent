# 文件清单

本资料包只服务于 **paperclip**，与另一个宿主资料包没有依赖关系。

路径均相对于公司宿主仓库根目录；A = 新增完整文件，M = 现有文件增量补丁。

| 类别 | 状态 | 宿主目标路径 | 包内交付位置 |
| --- | --- | --- | --- |
| runtime | M | `cli/package.json` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `cli/src/adapters/registry.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `packages/adapter-utils/src/acpx-engine/constants.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `packages/adapter-utils/src/acpx-engine/execute.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | A | `packages/adapters/tomcat-local/package.json` | [`new-files/packages/adapters/tomcat-local/package.json`](new-files/packages/adapters/tomcat-local/package.json) |
| runtime | A | `packages/adapters/tomcat-local/src/cli/index.ts` | [`new-files/packages/adapters/tomcat-local/src/cli/index.ts`](new-files/packages/adapters/tomcat-local/src/cli/index.ts) |
| runtime | A | `packages/adapters/tomcat-local/src/index.ts` | [`new-files/packages/adapters/tomcat-local/src/index.ts`](new-files/packages/adapters/tomcat-local/src/index.ts) |
| runtime | A | `packages/adapters/tomcat-local/src/server/acp.ts` | [`new-files/packages/adapters/tomcat-local/src/server/acp.ts`](new-files/packages/adapters/tomcat-local/src/server/acp.ts) |
| runtime | A | `packages/adapters/tomcat-local/src/server/config-schema.ts` | [`new-files/packages/adapters/tomcat-local/src/server/config-schema.ts`](new-files/packages/adapters/tomcat-local/src/server/config-schema.ts) |
| tests | A | `packages/adapters/tomcat-local/src/server/execute.test.ts` | [`tests/new-files/packages/adapters/tomcat-local/src/server/execute.test.ts`](tests/new-files/packages/adapters/tomcat-local/src/server/execute.test.ts) |
| tests | A | `packages/adapters/tomcat-local/src/server/fixtures/resume-agent.mjs` | [`tests/new-files/packages/adapters/tomcat-local/src/server/fixtures/resume-agent.mjs`](tests/new-files/packages/adapters/tomcat-local/src/server/fixtures/resume-agent.mjs) |
| runtime | A | `packages/adapters/tomcat-local/src/server/index.ts` | [`new-files/packages/adapters/tomcat-local/src/server/index.ts`](new-files/packages/adapters/tomcat-local/src/server/index.ts) |
| tests | A | `packages/adapters/tomcat-local/src/server/resume.test.ts` | [`tests/new-files/packages/adapters/tomcat-local/src/server/resume.test.ts`](tests/new-files/packages/adapters/tomcat-local/src/server/resume.test.ts) |
| tests | A | `packages/adapters/tomcat-local/src/server/skills.test.ts` | [`tests/new-files/packages/adapters/tomcat-local/src/server/skills.test.ts`](tests/new-files/packages/adapters/tomcat-local/src/server/skills.test.ts) |
| runtime | A | `packages/adapters/tomcat-local/src/server/skills.ts` | [`new-files/packages/adapters/tomcat-local/src/server/skills.ts`](new-files/packages/adapters/tomcat-local/src/server/skills.ts) |
| tests | A | `packages/adapters/tomcat-local/src/server/test.test.ts` | [`tests/new-files/packages/adapters/tomcat-local/src/server/test.test.ts`](tests/new-files/packages/adapters/tomcat-local/src/server/test.test.ts) |
| runtime | A | `packages/adapters/tomcat-local/src/server/test.ts` | [`new-files/packages/adapters/tomcat-local/src/server/test.ts`](new-files/packages/adapters/tomcat-local/src/server/test.ts) |
| tests | A | `packages/adapters/tomcat-local/src/ui/index.test.ts` | [`tests/new-files/packages/adapters/tomcat-local/src/ui/index.test.ts`](tests/new-files/packages/adapters/tomcat-local/src/ui/index.test.ts) |
| runtime | A | `packages/adapters/tomcat-local/src/ui/index.ts` | [`new-files/packages/adapters/tomcat-local/src/ui/index.ts`](new-files/packages/adapters/tomcat-local/src/ui/index.ts) |
| runtime | A | `packages/adapters/tomcat-local/tsconfig.json` | [`new-files/packages/adapters/tomcat-local/tsconfig.json`](new-files/packages/adapters/tomcat-local/tsconfig.json) |
| tests | A | `packages/adapters/tomcat-local/vitest.config.ts` | [`tests/new-files/packages/adapters/tomcat-local/vitest.config.ts`](tests/new-files/packages/adapters/tomcat-local/vitest.config.ts) |
| runtime | M | `packages/shared/src/constants.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `pnpm-lock.yaml` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `scripts/run-vitest-stable.mjs` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| runtime | M | `server/package.json` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/src/adapters/builtin-adapter-types.ts` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `server/src/adapters/registry.test.ts` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| runtime | M | `server/src/adapters/registry.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/src/services/agent-identity.ts` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `server/src/services/conversation-continuation.test.ts` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| runtime | M | `server/src/services/conversation-continuation.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `tsconfig.json` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `ui/package.json` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `ui/src/adapters/adapter-display-registry.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `ui/src/adapters/registry.ts` | [`existing-files.patch`](existing-files.patch) |
| tests | A | `ui/src/adapters/tomcat-local/index.test.ts` | [`tests/new-files/ui/src/adapters/tomcat-local/index.test.ts`](tests/new-files/ui/src/adapters/tomcat-local/index.test.ts) |
| runtime | A | `ui/src/adapters/tomcat-local/index.ts` | [`new-files/ui/src/adapters/tomcat-local/index.ts`](new-files/ui/src/adapters/tomcat-local/index.ts) |
| runtime | M | `ui/src/adapters/use-adapter-capabilities.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `ui/src/components/RuntimeTestCard.tsx` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `ui/src/components/TaskChatThread.test.tsx` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| runtime | M | `ui/src/components/TaskChatThread.tsx` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `ui/src/components/new-agent/NewAgentSetup.tsx` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `ui/src/components/task-chat/composer-run-settings.ts` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `ui/src/lib/agent-setup-fields.ts` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `ui/src/pages/NewAgent.test.tsx` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| tests | M | `vitest.config.ts` | [`tests/existing-files.patch`](tests/existing-files.patch) |

## 来源

- 宿主基线：`2f6ad2226aec330251438c47cbc45fd92e0d3132`
- 接入提交：`1f010b1655b85ec4cfcf4ca45114e8fa899a601c`
- Tomcat代码：`401b6f0cbd8be8478b5453815450f7ce6ada00e7`

完整哈希与补丁目标文件见 `manifest.json`。
