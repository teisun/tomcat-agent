| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-07-11 08:47 +0800 | ACTIVE | feature/add-model-form-redesign | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 后端 `source` 语义修复：`ModelCatalog` 新增 `builtin_ids` / `is_builtin_seed`；`ModelView.source` 按出厂 seed 判定 Builtin，与「能否删除」(`is_user_model`) 解耦；`tomcat init` 抄入 `models.toml` 的官方模型不再误标为 user @2026-07-10
- [✓] **[P0]** Add Model UI：分段控件改为标准 Tab（tablist/tab/tabpanel + 方向键）；空官方预置时默认落中转站 Tab 并给出引导；`saveDisabled` 旁显示原因；`findMatchingProviderPreset` 放宽为 provider(+api) 匹配 @2026-07-10
- [✓] **[P0]** 双锚点表单落地：官方 preset（模式 A）与 relay/base_url 派生（模式 B）；`thinking_format=Auto` 严格按 wire `api` 推断，不再被 model_name 误导 @2026-07-10
- [✓] **[P1]** 验收门禁：`cargo test`、扩展 `build` / `test:unit` / `test:e2e:vscode-devhost` 全绿；手测清单与 hostE2e 场景已更新；纯插件包 `tomcat-vscode-ext-0.1.5.vsix` 已重打 @2026-07-10
- [✓] **[P2]** 测试稳定性：`TempHomeGuard` 加进程内锁，消除全量 `cargo test` 下 HOME 并发竞态；Auto thinking 断言对齐默认 `high` effort @2026-07-10

### 🔌 INTERFACE (接口变更)
- `ModelCatalog::is_builtin_seed(id)` / `SharedModelCatalog::is_builtin_seed(id)`：新增；按内嵌 `builtin_models.toml` 出厂清单判定。
- `ModelView.source`：语义从「是否在用户 models.toml」改为「是否在出厂 seed 清单」；GUI Delete 可见性随之变化（seeded 官方模型隐藏 Delete）。
- `ThinkingFormat::Auto`：运行时仅按 wire `api` 解析；`resolve_for_model` 仅保留给测试/工具路径。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 集成说明
- 本分支目标：修复 Add Model「Provider 下拉空白」根因 + 两模式 UI Tab 化，并完成双锚点表单整改。
- 行为说明：被 `tomcat init` 抄入文件的官方模型，UI Delete 从「显示」变为「隐藏」；CLI `tomcat model remove` 仍可删文件覆盖项并回落到内嵌出厂默认。
- 验收：`cargo test`；`npm run build` + `npm run test:unit` + `npm run test:e2e:vscode-devhost`；`npm run package:vsix` → `tomcat-vscode-ext/tomcat-vscode-ext-0.1.5.vsix`（gitignore，不入库）。
