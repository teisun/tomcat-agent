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

## 来源

- 宿主基线：`06835b8f50606ca3f8bebbb5d07ac8901f831e69`
- 接入提交：`e12a45e59d8bfddf2aba192f163899a9eb290a3b`
- Tomcat代码：`401b6f0cbd8be8478b5453815450f7ce6ada00e7`

完整哈希与补丁目标文件见 `manifest.json`。
