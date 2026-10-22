### develop 状态入口

本分支的唯一状态表维护在 [tomcat/docs/status/develop.md](../../tomcat/docs/status/develop.md)，与现有状态汇总目录保持一致；元数据、Cov%、DONE、INTERFACE 和 BLOCKED 均以该文件为准，此处不维护副本。

2026-10-22，共享后台与 Paperclip/Multica ACP 接入已完成本机自动化及真实双宿主验收，Tomcat 已推送 `feature/shared-serve-acp`；当前继续修复、复跑 GitHub Windows 门禁，尚不称三平台全绿。最新修订、分项结果及证据以唯一状态表为准。不升版、不合并主干、不发布，宿主改动仅留本地分支。

2026-10-19 11:36 +0800，CLI `0.1.80` / EXT `0.1.96` 标准发版完成，新 EXT 内置 CLI `0.1.80`。发布提交 `845416e664ce55aff8304a3ad20f5882e7cdcc25` 已推送 develop/main/master，两个标签均指向它；先确认 CLI 四平台资产齐全，再发布 EXT。GitHub 四份 CLI 归档、五份 VSIX 及各自 SHA256SUMS 已公开，9 份下载包哈希通过；四平台原生安装验收通过，包内 CLI 与发布归档一致，CHANGELOG 与发布提交一致。Marketplace 四平台已公开且全部 Validated，商店 SHA256 与 GitHub 一致；OpenVSX 跳过。本地未额外跑产品测试、构建或安装；main 普通 CI 已通过，develop Windows Rust 尚在运行，不称所有分支全绿。详情与证据见唯一状态表及 `.agents/acceptance/release-0.1.80-0.1.96/release.md`，Cov% 未测量。

2026-10-19 11:07 +0800，IP 中转站自动凭证槽修复及回归测试纳入本次本地提交：仅数字开头的派生名添加 `PROVIDER_`，严格校验、provider 与模型 ID 不变；旧默认槽需重新配置 Key，不自动迁移。此前聚焦测试 GUI 35、auth 7、CLI 5、隔离环境后 admin 28 项通过，宽窄屏 4 套截图/ARIA/console 已检查；首轮 admin 的环境变量干扰与模拟宿主边界保留在唯一状态表。按用户要求本次不跑测试、不构建，Cov% 未测量；未升版、未推送或发布。本地验收记录 `.agents/acceptance/ip-relay.md` 不强制纳入 Git。

2026-10-19 08:30 +0800，CLI `0.1.79` / EXT `0.1.95` 标准发版完成，EXT 内置 CLI `0.1.79`。发布提交 `ca1542117494d7f36a7a310b2d86cd69121a0007` 已推送 develop/main/master，两个标签均指向该提交；先确认 CLI 四平台资产齐全，再发布 EXT。GitHub 四份 CLI 归档、五份 VSIX 及各自 SHA256SUMS 已公开，9 份下载包校验通过；四平台原生安装验收通过，包内 CLI 与对应归档一致，CHANGELOG 与发布提交一致。Marketplace 四平台已公开且全部 Validated，哈希与 GitHub 一致；OpenVSX 跳过。本地未额外跑产品测试、构建或安装。普通 CI 仍有已知 QuickJS 用例失败，不称全量全绿；详情与证据见唯一状态表及 `.agents/acceptance/release-0.1.79-0.1.95/release.md`，Cov% 未测量。

2026-10-17 13:27 +0800，补齐插件 `0.1.92` / `0.1.93` 的 CHANGELOG，并在 `/release-cli-ext` 命令中明确发版前更新、核对版本归属和内置 CLI、确认日志进入 release 提交的要求；漏项时停止发版。两项文档及本次状态记录一并纳入本地提交。此前已核对发布标签、CLI pin、历史条目完整性和命令结构，本次不重跑产品测试或构建；不升版、不推送、不打标签或发布，既有资产不替换。已发布包仍含旧 CHANGELOG，补录内容须随后续插件包交付。CLI `0.1.77` / EXT `0.1.93` 的既有发布结果与环境告警保留在唯一状态表；Cov% 未测量。

2026-10-17 13:17 +0800，CLI `0.1.77` / EXT `0.1.93` 标准发版完成，新 EXT 内置 CLI `0.1.77`。发布提交 `8f88e8f8725249a9bea7cb653192ba2189bccd84` 已推送 develop/main/master，两个标签均指向它；仅使用快进，先确认 CLI 资产齐全再发布 EXT。CLI 三平台归档、EXT 四份 VSIX 及各自 SHA256SUMS 已公开，7 份下载包校验通过；EXT 三平台冷启动／0.1.90 升级门禁均通过，内置 CLI 与对应归档字节一致，纯包不含 CLI。Marketplace 三平台上传成功，13:16 +0800 公开查询均为已验证的 0.1.93，包哈希与 GitHub 一致。未额外重跑本地产品测试、安装或替换本机 CLI；本次收尾只回填状态，不移动标签、不重建或替换发布包。CI 环境告警与证据边界见唯一状态表及 `.agents/acceptance/release-0.1.77-0.1.93/release.md`；Cov% 未测量。

2026-10-16 21:43 +0800，「三项整改」与供应商分卡改动纳入本次本地提交。三项分别是：内置技能改由构建脚本生成嵌入表，开发机上的 `node_modules` 不再进入二进制，release 体积回到约 16.5MB；复核供应商分卡实现并修 4 处；Agent 提问时选中项改为金色字母块，与悬停明显区分。验收中另发现并修复一处测试竞态：代理用例清空 `NO_PROXY`，导致并发的型号列表单测失败。Rust lib/integration/Clippy/release、扩展 lint 与 GUI/core 单测、提问面板 9 场景视觉脚本均通过。这些是此前验收记录；按用户要求，本次提交不重新运行测试、构建或视觉验收，Cov% 未测量。本次未升版本、未推送或发布；截图未在真实 VS Code 安装包中复拍。详情见唯一状态表与 `.agents/acceptance/remediation-20261016/acceptance.md`。

2026-10-16 下午，「模型设置按供应商管理与内置目录更新」完成实施与验收，覆盖五项：供应商分卡含按型号选协议、连接供应商与首个配置即默认、内置目录 13 项、删除 Ultrafast、辅助模型。验收包括 Rust 全量门禁、扩展单测/集成、fake 安装 E2E、macOS x64 真实包冷启动与 0.1.90 升级、42 张宽窄视觉截图，以及用本机 Key 实拉 3 家型号列表（Key 未打印）。验收完成时未提交，本次一并纳入本地提交；未升版本、未推送或发布；Windows/Linux 原生安装与真实付费对话未验证。详情见唯一状态表与 `.agents/acceptance/vendor-model-settings/acceptance.md`。

2026-10-16 五项整改、拒写历史 `Created` 误标修复及 ask_question「执行中仅必要时提问」提示随本次本地提交纳入。项目脚本一次升级 CLI `0.1.74 → 0.1.75`、插件 `0.1.90 → 0.1.91`、内置 CLI `0.1.74 → 0.1.75`，前后版本校验与版本文件空白检查通过。按用户要求本次不跑测试、不构建/打包/安装、不推送/发布；此前源码验收结果与未解决的 QuickJS 失败保留，不称新版全绿。详情见唯一状态表与 `.agents/acceptance/denied-history/acceptance.md`。

2026-10-15 五项整改、自动化补缺、macOS x64 真实安装、许可的本机数据副本读取烟测及 20 张视觉截图交付已完成，状态 DONE，当时尚未提交或发布；模型设置属于独立计划。本机烟测保留原件只读，未读取原始受保护配置/凭证，不冒称全目录或真实供应商认证验证。详细结果、拆跑证据与已知问题见唯一状态表；CLI 0.1.74 / EXT 0.1.90 的既有三平台 GitHub 发版只作历史证据，不充当本轮通过证明。
