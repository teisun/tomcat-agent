# 文件清单

本资料包只服务于 **multica**，与另一个宿主资料包没有依赖关系。

路径均相对于公司宿主仓库根目录；A = 新增完整文件，M = 现有文件增量补丁。

| 类别 | 状态 | 宿主目标路径 | 包内交付位置 |
| --- | --- | --- | --- |
| runtime | M | `packages/core/types/agent.ts` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `packages/views/runtimes/components/runtime-profile-catalog.test.ts` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| tests | M | `packages/views/runtimes/components/runtime-profiles-dialog.test.tsx` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| tests | M | `scripts/agent-cli-command-names.txt` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| runtime | M | `server/internal/daemon/agents_probe.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/internal/daemon/config.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/internal/daemon/daemon.go` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `server/internal/daemon/config_test.go` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| runtime | M | `server/internal/daemon/execenv/context.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/internal/daemon/execenv/runtime_config.go` | [`existing-files.patch`](existing-files.patch) |
| tests | M | `server/internal/daemon/execenv/runtime_config_test.go` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| tests | M | `server/internal/daemon/execenv/sidecar_manifest_test.go` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| tests | A | `server/internal/daemon/execenv/tomcat_test.go` | [`tests/new-files/server/internal/daemon/execenv/tomcat_test.go`](tests/new-files/server/internal/daemon/execenv/tomcat_test.go) |
| tests | A | `server/internal/daemon/tomcat_test.go` | [`tests/new-files/server/internal/daemon/tomcat_test.go`](tests/new-files/server/internal/daemon/tomcat_test.go) |
| tests | A | `server/internal/migrations/tomcat_migration_test.go` | [`tests/new-files/server/internal/migrations/tomcat_migration_test.go`](tests/new-files/server/internal/migrations/tomcat_migration_test.go) |
| runtime | A | `server/migrations/584_runtime_profile_add_tomcat.down.sql` | [`new-files/server/migrations/584_runtime_profile_add_tomcat.down.sql`](new-files/server/migrations/584_runtime_profile_add_tomcat.down.sql) |
| runtime | A | `server/migrations/584_runtime_profile_add_tomcat.up.sql` | [`new-files/server/migrations/584_runtime_profile_add_tomcat.up.sql`](new-files/server/migrations/584_runtime_profile_add_tomcat.up.sql) |
| runtime | M | `server/pkg/agent/agent.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/pkg/agent/launch.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/pkg/agent/models.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | A | `server/pkg/agent/tomcat.go` | [`new-files/server/pkg/agent/tomcat.go`](new-files/server/pkg/agent/tomcat.go) |
| tests | A | `server/pkg/agent/tomcat_integration_test.go` | [`tests/new-files/server/pkg/agent/tomcat_integration_test.go`](tests/new-files/server/pkg/agent/tomcat_integration_test.go) |
| tests | A | `server/pkg/agent/tomcat_test.go` | [`tests/new-files/server/pkg/agent/tomcat_test.go`](tests/new-files/server/pkg/agent/tomcat_test.go) |
| runtime | M | `server/pkg/agent/version.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/pkg/agent/thinking.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | M | `server/pkg/taskfailure/failure.go` | [`existing-files.patch`](existing-files.patch) |
| runtime | A | `server/pkg/agent/tomcat_models.go` | [`new-files/server/pkg/agent/tomcat_models.go`](new-files/server/pkg/agent/tomcat_models.go) |
| tests | A | `server/pkg/agent/tomcat_models_test.go` | [`tests/new-files/server/pkg/agent/tomcat_models_test.go`](tests/new-files/server/pkg/agent/tomcat_models_test.go) |
| tests | A | `server/pkg/agent/tomcat_fixture_test.go` | [`tests/new-files/server/pkg/agent/tomcat_fixture_test.go`](tests/new-files/server/pkg/agent/tomcat_fixture_test.go) |
| tests | M | `server/internal/daemon/model_list_binary_test.go` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| tests | A | `server/internal/daemon/tomcat_models_test.go` | [`tests/new-files/server/internal/daemon/tomcat_models_test.go`](tests/new-files/server/internal/daemon/tomcat_models_test.go) |
| tests | A | `e2e/tomcat-model-discovery.spec.ts` | [`tests/new-files/e2e/tomcat-model-discovery.spec.ts`](tests/new-files/e2e/tomcat-model-discovery.spec.ts) |

共 **32 个目标路径**：原包 31 项全部保留，新增 `thinking.go` 一行能力登记。`tomcat.go`、`tomcat_models.go` 与相关测试接通目录档位、默认模型、先模型后强度、Default 清除和失败不 prompt。原清理、取消、启动策略与人工等待测试继续保留。原基线没有新版 `thinking_level_warning_test.go`：本次四项选择校验在已有 `tomcat_models_test.go` 中验证原日志告警/丢弃行为，不导入新版告警机制。E2E 面向当前 `5e36721c` 基线加工作树，不导入新版 UI。

## 来源

- 宿主基线：`06835b8f50606ca3f8bebbb5d07ac8901f831e69`
- 接入提交：`e12a45e59d8bfddf2aba192f163899a9eb290a3b`
- 本轮配套 Tomcat 源码：`bfea42a7625fee14c1a74e83e2ff471bd8209fb3` 基线加未提交的强度功能工作树；核心文件哈希见 `manifest.json.host.reasoning_effort`。上一轮配套为 `9d7221317e272645128d090bc38151f943d6a2aa`，原接入依赖为 `401b6f0cbd8be8478b5453815450f7ce6ada00e7`。
- 历史目录修复来源：Multica `3c7b4cde72ab4c8389dcec933e2573855b04cb7e` 的已审工作树；Tomcat `1f0fbf0c` 基线上的当时工作树（当时未提交，不代表本轮状态）。`manifest.json.host.catalog_repair` 保留原修复来源；逐文件 SHA-256 是当前交付内容的精确标识。
- 上一轮 adapter 来源：Multica `5e36721c1329372d272e8c4b72f4ada2e31115a7`；原清理接线仅语义回迁必要行为。本轮在该基线上回迁三个生产文件的工作树差异；精确来源与每个目标文件哈希记录在 `manifest.json`，不声称存在未提交的新提交。不导入 Pi、桌面、实时 usage、调度或 native CLI 停机确认扩展。
- `tomcat_test.go` 沿用该提交并将配置失败分类测试放入默认单测；daemon 按旧基线的显式路径约定检查原路径与 Command，按真实 snake_case wire 捕获取消回报，加入 handoff/清理结果/两条取消回报回归；受控集成测试按真实服务版本核对 observer 身份，保留不同版本服务测试选项。

完整哈希与补丁目标文件见 `manifest.json`。
