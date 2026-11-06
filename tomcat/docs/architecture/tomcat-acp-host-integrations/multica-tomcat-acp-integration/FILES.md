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
| runtime | A | `server/pkg/agent/tomcat_models.go` | [`new-files/server/pkg/agent/tomcat_models.go`](new-files/server/pkg/agent/tomcat_models.go) |
| tests | A | `server/pkg/agent/tomcat_models_test.go` | [`tests/new-files/server/pkg/agent/tomcat_models_test.go`](tests/new-files/server/pkg/agent/tomcat_models_test.go) |
| tests | A | `server/pkg/agent/tomcat_fixture_test.go` | [`tests/new-files/server/pkg/agent/tomcat_fixture_test.go`](tests/new-files/server/pkg/agent/tomcat_fixture_test.go) |
| tests | M | `server/internal/daemon/model_list_binary_test.go` | [`tests/existing-files.patch`](tests/existing-files.patch) |
| tests | A | `server/internal/daemon/tomcat_models_test.go` | [`tests/new-files/server/internal/daemon/tomcat_models_test.go`](tests/new-files/server/internal/daemon/tomcat_models_test.go) |
| tests | A | `e2e/tomcat-model-discovery.spec.ts` | [`tests/new-files/e2e/tomcat-model-discovery.spec.ts`](tests/new-files/e2e/tomcat-model-discovery.spec.ts) |

共29个目标路径：保留原23项，增加上述6项；原 `tomcat_test.go` 和受控集成测试已跟随真实目录更新。E2E源码面向3c7b4cde7页面/测试API，未将该版本的UI或其它业务改动倒入旧基线。新增 `tomcat_fixture_test.go` 仅给受控协议测试提供封闭HOME/环境，避免继承开发账号；不是业务实现。

## 来源

- 宿主基线：`06835b8f50606ca3f8bebbb5d07ac8901f831e69`
- 接入提交：`e12a45e59d8bfddf2aba192f163899a9eb290a3b`
- Tomcat代码：`401b6f0cbd8be8478b5453815450f7ce6ada00e7`
- 目录修复来源：Multica `3c7b4cde72ab4c8389dcec933e2573855b04cb7e` 的已审工作树；Tomcat `1f0fbf0c` 基线上的本次工作树（未提交）。`manifest.json.host.catalog_repair` 记录原来源与修复来源；逐文件SHA-256是交付内容的精确标识。
- 原基线重放后，本轮新增生产差异仅 `server/pkg/agent/models.go` Tomcat分支和新增 `tomcat_models.go`。不包含用户启动器、daemon/API/页面/公共查询或通知增量。

完整哈希与补丁目标文件见 `manifest.json`。
