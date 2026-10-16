# Tomcat 发布到 VS Code Marketplace

> 最初调研于 2026-10-12；2026-10-13 完成 0.1.89 网页首发与 Intel Mac 市场安装，2026-10-14 网页更新 0.1.90。2026-10-16 经用户授权开启自动发布，EXT 0.1.92（内置 CLI 0.1.76）已通过真实 OIDC 发布至市场三平台，公开 hash 与 GitHub/原生验收一致；保留安装通过 VS Code 正常更新命令从 0.1.89 更新至 0.1.92。未将此说成无人值守定时更新或真人首配/付费对话验收。

> 2026-10-14 发版选择：CLI 0.1.74 / EXT 0.1.90 的原工作流只发布 GitHub Release，市场随后人工上传相同已验收包，未重跑 CI。`publish` 不依赖 Azure；独立 `marketplace` job 仅在仓库 Actions 变量 `PUBLISH_MARKETPLACE` 严格等于 `true` 时启用。当时未设置该变量；2026-10-16 已显式设为 `true`，0.1.92 首次自动上传成功。

```text
确认 tomcat-agent Owner → 补全发布者 Logo/资料
  → 对齐扩展身份和 README → 构建并原生安装三平台正式包
  → GitHub Release → 网页上传 → 等待市场校验 → 市场安装
  → Entra 联邦身份 → 后续正式标签发布 → 已有安装接收更新
```

## 1. 身份、账号和公开资料

- Publisher：`tomcat-agent`，已有账号，管理页已确认 Owner。不要再创建另一个发布者。
- 扩展机器名：`tomcat-vscode-ext`；完整 ID：`tomcat-agent.tomcat-vscode-ext`；展示名称：**Tomcat AI Agent**。首次上传的原显示名 Tomcat 被市场以 HTTP 400 拒绝（`This extension display name is taken`）；用户确认的新名称已被市场接受。只改显示名，不改 Publisher/name、命令、设置或 Agent Box 名称。
- 版本由 `release-versions.json` 管理；本次市场首发为 EXT 0.1.89，CLI/pin 保持 0.1.73。GitHub 的 0.1.88 已公开，保留其资产和标签，不用修改后的包覆盖旧版本。不要手改版本镜像或运行 `npm version` / `vsce publish patch`。
- 扩展图标是 `tomcat-vscode-ext/media/icon.png`（256×256 PNG）。Publisher 的 Logo 在网页单独上传；使用它的 128×128 副本，不覆盖源图。
- Publisher 简介：Tomcat is an open-source AI agent runtime and VS Code extension for coding workflows, with configurable models and permission-controlled tools.
- Company website / Source code repository：<https://github.com/teisun/tomcat-agent>；Support：<https://github.com/teisun/tomcat-agent/issues>。没有提供的社交账号留空，不公布个人邮箱。
- 保存后刷新管理页并检查 View profile，不能以选中图片或点击 Save 作为保存成功证据。
- 蓝色认证勾不是首次发布前提。不要填不属于用户的域名或为此创建公司/网站。

## 2. 展示和支持范围

最低要求为 **VS Code 1.106.0**：右侧栏 view container 在该版本进入稳定扩展 API。1.104/1.105 用户先升级；不启用 proposed API，也不增加另一套侧栏实现。

市场详情正文由包内 README 生成。双语 README 使用公开 HTTPS 截图、语言与包外文档链接；实际打包后仍检查其地址和市场渲染。最初检查的旧包截图地址被 vsce 改成包含 `HEAD/../assets` 的地址并返回 404；不是断言所有相对链接都不可用。

市场只上传三个同身份、同版本、不同目标的内置包：

| VSIX target | CLI asset target | 原生验收 |
|---|---|---|
| darwin-arm64 | aarch64-apple-darwin | Apple Silicon macOS |
| darwin-x64 | x86_64-apple-darwin | Intel macOS |
| linux-x64 | x86_64-unknown-linux-gnu | Linux x64 / glibc |

通用 VSIX 不含 CLI，仅在 GitHub 留存，不向市场提供 universal 回退。Windows、Linux ARM64、Alpine/musl、远程开发和 vscode.dev 不在本次支持承诺内；记录实际 OS，不能以一台 Ubuntu 验收宣称所有发行版支持。

分类使用官方清单明确允许的 Other，AI 保留搜索关键词；不是声称服务器已拒绝过旧 AI/Chat 分类。保留 MIT、PNG 和双语资源；说明模型/API 单独费用、所配置服务接收的消息/上下文/工具结果和权限门控，不承诺完全本地、免费或零遥测。

## 3. 打包与安装门禁

```bash
# 根目录：EXT-only，不改变 pin
node scripts/release-version.mjs check
# 本次显示名修正已递增至 0.1.89；不要重复递增。
# 仅准备下一正常扩展版本时执行：
# node scripts/release-version.mjs bump --extension patch
node scripts/release-version.mjs check

# 扩展目录：仅在准备新版本时执行，避免重复递增
npm ci
npm --prefix gui ci
npm run gate:full
npm run package:vsix -- --target darwin-x64 --bundle-binary /absolute/path/to/verified-cli/tomcat
```

其他平台必须换用对应的 CLI；从 pin 的 CLI Release 下载并核对 SHA256SUMS，不将本机二进制复制进其他 target。沿用 `.cursor/commands/release-cli-ext.md` 的提交、FF 分支同步和标签顺序；EXT-only 无需重发已存在的 CLI Release。不要 force push 或移动已发布标签。

现有 `gate:full` / 默认 `verify:vsix` 使用 fake serve，证明安装/UI 回归但不证明正式二进制可用。正式模式需要消费同一批 exact VSIX、不重打包替换 CLI，并在对应原生宿主验证身份/version/target、内置 CLI hash/version、真实握手、模型请求、工具允许/拒绝、停止和重载。模型响应可由本地 HTTP fixture 控制，CLI 和宿主必须真实。记录 PNG、ARIA、console 和相关 webview errors。正式产物验证尚未通过时，不上传。

首配整改的原生验收从**同一个干净 HOME**开始：不 init、不预写模型或 Key、不预设默认/压缩模型覆盖。先断言服务已连接、主配置尚不存在、配置卡可见且没有初始化弹框；通过真实 Settings 控件添加指向本地 HTTP 的模型、Key 并设默认，验证同一 PID 恢复发送，再检查重载后的选择保持。原有工具权限场景需要启动时加载专用测试策略，因此只在首条同进程请求已成功之后，将策略加入该测试 HOME 并重启继续权限/停止回归；不能用这次重启冒充模型配置的生效方式。

本轮源码验收必须打入本次构建的真实 CLI；旧 pin 的已发布包不能证明这次修复。`TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR` 指向 `.agents/acceptance/onboarding`（从仓库根目录解析），保留未配置卡、模型表单、配置后、窄屏与失败恢复的 PNG、ARIA、console。读取证据后再登记通过；这里的流程要求不代表已经完成验收，也不验证真实外部供应商认证。

用户已明确选择先跑通发布链路、后续版本再修复历史标签缺陷，并于 2026-10-14 明确允许 CLI 0.1.74 延续同一已知问题。例外仅限 CLI 0.1.73 / 0.1.74：重载后被拒写入可能显示 Created。验收仍须证明真实拒绝、文件未创建（含重载后）、允许写入、停止/恢复及包身份/架构/载荷正确；缺失历史状态和其它 CLI pin 不得享受此例外。回执使用 `passed_with_known_issues` 并列明问题，双语 README/CHANGELOG 公开说明；不把它记录为已修复或完全视觉通过。此前失败回执保留。

```bash
# 对应原生平台：直接验证已经生成的正式包，不重新构建或替换 CLI。
npm run verify:vsix -- --release-bundle /absolute/path/to/exact-platform-package.vsix

# 仅在真实上架后：按当前仓库的精确扩展版本从市场安装并复用同一验收。
npm run verify:vsix -- --marketplace tomcat-agent.tomcat-vscode-ext@0.1.89
```

这两种来源互斥，错误文件不能被当作市场 ID 或被旧的 `TOMCAT_PREBUILT_VSIX` 替代。正式模式默认下载清单所声明的最低 VS Code 版本；保留文件、CLI 与原生架构证据。受控模型返回不是外部模型服务的实际调用，配置规则的允许/拒绝也不代替首次市场安装中的人工授权交互验收。

## 4. 首次网页发布

首次不依赖 PAT 或 Entra：
1. 确认三平台正式包和 UI 验收都通过，保留提交 SHA、文件 hash、目标与日志。
2. 在 <https://marketplace.visualstudio.com/manage> 选择 tomcat-agent → New extension → Visual Studio Code。
3. 上传第一个正式平台 VSIX，再为同一版本添加其余目标。菜单以当时后台为准；不能上传旧 Publisher 的包或通用包。
4. 等待市场校验，处理明确错误；没有固定完成时限。真实权限或名称冲突按服务端提示处理。
5. 打开 <https://marketplace.visualstudio.com/items?itemName=tomcat-agent.tomcat-vscode-ext>，检查图标、简介、截图和语言/文档链接。该页面已公开显示 0.1.89 和三个目标；之后发版仍须重新核实。
6. 在干净 VS Code 从市场按 ID 安装，核对版本、平台和内置 CLI，完成首次配置及主流程。

### 0.1.89 首发实况与验收边界

- 管理页显示绿色勾、`0.1.89`、`Public`、`Works with: 3`；将鼠标移到 `3` 上可看到 Linux x64、macOS Apple Silicon、macOS Intel。同一扩展 ID 的三个目标包合并成一行，VS Code 安装时自动匹配平台。
- 公开页图标、README 截图、双语入口、版本和支持范围已核实。Marketplace 自身仍报告字体 CSP 和站点资源 404，保留控制台证据，不宣称整页控制台零错误。
- 本机 Intel macOS / VS Code 1.106.0 已从市场按精确 ID/版本完成隔离安装；实际来源为 gallery、目标为 darwin-x64，内置 CLI 0.1.73 经过真实 Serve、受控模型响应、工具允许/拒绝、停止与重载检查，回执为 `passed_with_known_issues`。这不等于新用户通过 Add Models 配置真实外部服务的人工验收，也不是另外两个平台的市场安装证明。
- 0.1.89 仅改显示名/版本和说明。按用户明确要求，不再触发 CI；复用已完成的归档检查和本机正式包检查，不把此前 0.1.88 三平台原生证据冒充本版本三平台已重测。
- 网页首发不依赖 Entra；首发时暂停的长期配置现已恢复推进，最新状态见第 5 节。首发记录与证据保留在 `.agents/marketplace/0.1.89/`。

## 5. 后续联邦身份发布（0.1.92 已实际验证）

微软公告全局 PAT 在 **2026-12-01** 退役。首次上架不用 PAT，长期目标也不用 client secret/PAT；本仓库不再以“增加 VSCE_PAT”作为接通长期自动化的步骤。

```text
GitHub marketplace environment（仅正式 ext-v* tags）
  → GitHub OIDC → Entra 专用 application/service principal
  → azure/login → vsce --azure-credential
  → Publisher Contributor → 三个已验收包 → 市场更新
```

- 先确认用户租户和应用注册权限。优先已有租户、无订阅登录，不默认创建付费云资源；必需邮箱、MFA/条款由本人操作，涉及付费先确认。
- 联邦 issuer 为 GitHub Actions，audience 为 `api://AzureADTokenExchange`，subject 为 `repo:teisun/tomcat-agent:environment:marketplace`；环境部署规则只允许正式 ext-v* 标签，不授权任意 PR/fork。
- 通过官方 profile API 获取实际身份的 Marketplace profile ID并授予 Publisher Contributor；它不等于 client ID。Azure 登录成功也不证明 Publisher 写权限。
- 在 `release-ext` 中仅市场发布 job 具有 `id-token: write`；另有下述手动身份初始化任务使用同一联邦身份。固定受支持 azure/login 的版本/SHA，保持登录 cleanup；令牌与 secrets 不写入源码/命令历史/日志。
- 从相同提交按 npm lockfile 安装 vsce，运行 `npx --no-install vsce publish --azure-credential --packagePath <已验收文件>`。仅显式选择三个平台，缺包/认证错误必须失败，不静默跳过。
- `release-ext` 的 workflow_dispatch 和 dry-run 只打包验收，不执行市场上传。正式标签必须依赖 native gate 的成功；GitHub release 成功不等于市场成功。
- 用正常新版本验证实际发布与首发安装更新，不为认证测试制造版本。0.1.92 已完成真实联邦上传与保留 profile 的正常市场更新；YAML 配置或仅登录成功仍不能替代这些结果。

### 工作流实现与启用前提

`release-ext.yml::publish` 负责通过原生验收后的 GitHub Release，不需要 Azure 凭据；`marketplace` 是其后的独立可选 job。2026-10-14 已配置单租户应用 `tomcat-marketplace-publisher`、`github-marketplace` 联邦凭据及 GitHub 环境中的两个身份参数，门户显示证书 0、客户端密码 0、联邦凭据 1。**身份初始化已于 2026-10-14 成功；真实三平台 OIDC 上传已于 2026-10-16 在 [EXT run 38912460284](https://github.com/teisun/tomcat-agent/actions/runs/38912460284) 成功。**

Marketplace Members 要求其自己的 User/profile ID，不能填写应用 client ID 或 Entra 对象 ID。经用户明确授权的[仅身份初始化任务 38544950700](https://github.com/teisun/tomcat-agent/actions/runs/38544950700)成功：先核对 service principal、client ID 和 tenant ID，再调用官方 profile API 取得实际 profile ID。该身份已被授予 `tomcat-agent` 的 **Contributor**，完整刷新后仍可见；成员行的“租户 ID\\服务主体 ID”与刚创建的应用对应，原个人 Owner 保持不变。未配置 PAT/client secret、Azure 订阅 Owner 或其他云资源权限。此结果证明身份登录和授权配置，不代替真实上传验收。

本次还实查 GitHub OIDC 设置：`use_default=true`、`use_immutable_subject=false`，因此信任继续精确匹配 `repo:teisun/tomcat-agent:environment:marketplace`。不要直接套用 Entra 门户新模板中的 `@owner_id/@repository_id` 格式；仓库以后重命名、转移或启用 immutable subjects 时，须同步复核并迁移此信任，不添加通配或旧格式备用信任。配置及浏览器回读证据位于 `.agents/marketplace/entra-tenant-status.json`、`.agents/marketplace/github-environment-status.json`。

- 明确启用后才自动上传：先配齐下面的 OIDC 信任、凭据与 Publisher 授权，再由维护者把**仓库级** Actions 变量 `PUBLISH_MARKETPLACE` 设为 `true`。未设置或其它值均跳过市场 job；该开关不跳过 GitHub 发包前的三平台原生验收。
- GitHub `marketplace` environment 的 Secrets 存放 `AZURE_CLIENT_ID`、`AZURE_TENANT_ID` 两个身份标识；不是 client secret，不配置 PAT 或订阅 Owner 权限。已有环境标签规则仅允许 `ext-v*`；Entra 的精确 subject 必须保持上文值。
- GitHub `publish` 仍依赖 `build` 和全部 `verify`；只有可选 `marketplace` job 申请 `id-token: write`，且要求正式 `ext-v*` 标签、`PUBLISH_MARKETPLACE == 'true'` 与 `publish` 成功。它 checkout 本轮 `github.sha`，按仓库 lockfile 安装 `npm ci --ignore-scripts`，下载同一轮已验收 VSIX，不重打包。`azure/login` 固定为已核对的 v3 提交 `a641126d1b8aa4d1fa005f4f92df94a3a4c4c906`；后续升级需显式更新 SHA。
- 在任何发布前检查身份配置和三个精确文件全部存在且非空；缺项明确失败。按固定平台列表顺序上传，清除继承的 `VSCE_PAT`（vsce 3.9.2 会优先使用它，即使同时传 `--azure-credential`）；只用锁定工具的 `--packagePath`，不上传 universal，不用 `--skip-duplicate`，第一个失败立即停止。
- GitHub Release / Open VSX 原有逻辑保留；市场登录与上传在独立 job 中，未启用或失败都不会撤销已经创建的 GitHub Release。三渠道不是事务，不能把 GitHub 成功当作市场完成；启用后的市场认证或上传失败仍明确报错。
- 上传 shell 的既有用例保留：只替换上传命令边界，覆盖选包、缺第三包/缺认证时零上传、首个/中途失败停止和旧 PAT 不外泄。新增静态边界断言覆盖 GitHub job 无 Azure 依赖及市场显式启用；本轮按用户要求未在本地重跑测试。这些断言本身不证明微软实际授权；真实 OIDC 发布与已有安装正常更新的结果见下方 0.1.92 实证。

创建租户前先读[微软当前前提](https://learn.microsoft.com/en-us/entra/fundamentals/create-new-tenant)：管理中心内新增 Workforce 租户对付费客户有限制，免费/试用用户另有免费账号入口，不保证个人账号符合资格或无需账单验证。优先已有目录；账号、国家/组织信息、条款和费用由用户决定，不用面向客户登录的 External ID 试用目录替代本任务。

### 仅身份初始化（不发版）

用户已明确允许一次仅认证的 GitHub Actions 任务。`.github/workflows/marketplace-identity.yml` 只接受手动触发，复用同一个 `marketplace` 环境、身份参数和锁定的 Azure login；不 checkout、不构建、不跑产品测试、不上传包。它先核对登录类型、client ID 和 tenant ID，再仅输出校验过的 Marketplace profile UUID，不输出访问令牌。

首次运行使用非发布标签 `marketplace-identity-20261014`：它不匹配现有 CLI/EXT 发版触发器；任务提交使用 `[skip ci]` 避免普通分支推送触发 version-sync。该标签只获得一次临时的精确环境放行，任务结束即撤销，保留原 `ext-v*` 标签规则；不要扩大为任意分支或通配检查标签。身份任务成功本身不证明 Contributor 授权或实际上传成功；当时 `PUBLISH_MARKETPLACE` 保持关闭，后于 2026-10-16 经用户授权开启。

实际 run `38544950700` 仅包含身份配置检查、OIDC 登录、profile 查询及登录清理，全部成功；没有包构建、产品测试或上传。临时规则 `63011430` 已删除，API 回读仅保留原 `ext-v*` 规则 `62845495`。检查标签保留为审计引用，但不再允许进入该环境。权限页 PNG/ARIA/console 已读取；刷新后仅记录到 Google Tag Manager 超时，不宣称第三方门户控制台零错误。

### 后续发版究竟由什么触发

```text
改源码 + 递增 EXT 版本 + 推送 ext-v<新版本> 标签
  → release-ext：构建 → 三平台原生验收
  → GitHub Release 留存
  → 可选：PUBLISH_MARKETPLACE=true + OIDC 就绪 → Marketplace 上传
  → 市场校验/公开 → VS Code 从市场检查并更新
```

`release-ext.yml` 监听 `push.tags: ext-v*`；**手动把 VSIX 附到 GitHub Release、普通分支 push、单独发 CLI 标签，都不会自动更新 Marketplace**。首发及 0.1.90 更新为网页上传；0.1.92 已由开启后的正式标签工作流完成真实 OIDC 上传，无手工传包或 PAT。公开 Gallery API 确认三个平台版本及 SHA256 均与 GitHub/原生验收原包一致。保留的 0.1.89 Intel Mac 安装使用 VS Code 1.106.0 `--update-extensions` 正常更新至 0.1.92，无 `--force`、重装、VSIX 路径或精确版本 pin；内置 CLI hash 匹配 0.1.76。它证明已安装用户能经市场接收更新，不证明后台定时器的无人值守行为。

后续 EXT-only 使用既有 `node scripts/release-version.mjs bump --extension patch`，按仓库规范提交、推送、FF 同步，再推送对应正式 `ext-v<版本>` 标签。要更换内置 CLI，还要用版本脚本明确设置新 pin 和新 EXT 版本，并先确认 CLI Release 三份资产已就绪。保持同一 Publisher/name，不能覆盖已发布版本。市场公开后，已有安装只有在自动更新开启、VS Code 版本和目标平台兼容时才会自动更新；校验、检查和应用更新不是瞬间完成，也不承诺固定等待时长。

### 0.1.92 发布实证与边界

- 发布提交 `cd959b55fdc4d46db41b01ccf460289b30581ef8`；CLI run `38911202584`，EXT run `38912460284`。三平台冷启动与 0.1.90 数据升级原生验收均通过，GitHub/市场使用相同包。
- OIDC 三平台上传在 14:07 UTC 前后成功，公开市场至 14:16 UTC 才确认全部目标。上传成功与市场校验/公开是不同阶段；等待时不重复上传。
- 证据：`.agents/acceptance/release-0.1.76-0.1.92/` 的 `ext-run.log`、`package-verification.json`、`marketplace-public-final.json`、`marketplace-update-{before,after}.json` 及市场 PNG/ARIA/console。首次公开查询尚未看到新版的记录保留，不把它误记为凭据失败。
- 保留 profile 的真人首配/真实付费模型请求、无人值守定时更新和全部 CI 图片的本次人工复阅未完成。第三方页面字体 CSP/资源 404 与 Linux 宿主环境告警如实保留，不宣称零错误。

## 6. 部分失败和补发

逐目标上传可能部分成功。先查看后台每个 `(version,target)` 的状态，并比对原始已验收文件 hash；只补发缺失目标。已经成功上传的目标不重新打包，不用 skip-duplicate 隐藏结果，也不盲目重跑全集。

网页 Update / 添加平台使用同一版本的缺失目标；认证已就绪时，对缺失的一个文件执行 `vsce publish --azure-credential --packagePath <exact-file>`。需要代码修复则发布更高版本。不要删除市场扩展来“重来”：Remove 不可逆，名称可能永久保留。不能假设同版本可覆盖。

GitHub、Marketplace 和 Open VSX 是独立渠道。本次仅配置 `marketplace` 环境的 `AZURE_CLIENT_ID` / `AZURE_TENANT_ID`，未添加 VSCE_PAT、client secret 或 OVSX_PAT；不为了 VS Code 上架启用 Open VSX。若之后配置 OVSX_PAT，应由维护者确认该额外公开渠道。

## 7. 验收记录与网络边界

验收文件放在配置的项目资源目录（当前 `.agents/marketplace/`）；按版本/目标记录结果。源码检查、生成包安装、市场安装、OIDC 发布与已有安装更新是不同证据，不能相互冒充。

继承代理可能指向已关闭的 `127.0.0.1:58591`；可只对单条下载/查询移除进程代理变量，不改用户全局配置。微软登录解析落到本地地址时，可以临时采用 Chrome 安全 DNS 并清理浏览器 DNS 缓存，结束恢复原提供商；不能关闭浏览器网络安全检查。验证码由用户本人完成，不绕过 reCAPTCHA。

## 官方参考（执行前复核）

- [Publishing Extensions](https://code.visualstudio.com/api/working-with-extensions/publishing-extension)
- [Extension Manifest](https://code.visualstudio.com/api/references/extension-manifest)
- [Continuous Integration](https://code.visualstudio.com/api/working-with-extensions/continuous-integration)
- [全局 PAT 退役公告](https://devblogs.microsoft.com/devops/retirement-of-global-personal-access-tokens-in-azure-devops/)
- [GitHub → Azure OIDC](https://learn.microsoft.com/en-us/azure/developer/github/connect-from-azure-openid-connect)
- [Azure/login](https://github.com/Azure/login)
