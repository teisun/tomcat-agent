# 用户自定义系统提示词：缓存命中实测

日期：2026-10-04。基线提交：`aa648fe9`，CLI `0.1.66`。实测阶段只新增/接入测试探针及本报告，没有修改生产实现、真实规则、模型配置或版本。以下为提交前取得的结果，后续提交操作不重跑测试。

```text
本项目 3 条 alwaysApply 规则
          │ 生产 discover / render_rules（临时副本与原规则渲染逐字节相同）
          ▼
SystemPromptSnapshot
    ├─ A：不注入规则 ── 4 轮 ─┐
    └─ B：注入规则   ── 4 轮 ─┴─ 交替先后；相同 tools / 历史 / runtime tail
              │
              └─ 临时副本改一条规则 ── 3 轮，路由 key 不变
                                      │
                                      ▼
                          真实 provider 返回的缓存 token
```

## 1. DeepSeek 首轮结论（GPT-5.6 双网关补测见第 7 节）

**本次 `deepseek-v4-flash` 配置路由下，启用自定义规则没有表现出持续性缓存命中下降。** 排除各阶段第一轮后的 token 加权命中率如下：

| 场景 | 计入轮次 | 缓存读取 / 输入 token | 加权命中率 |
| :--- | :--- | ---: | ---: |
| A：关闭自定义规则 | 第 2–4 轮 | 36,608 / 37,122 | 98.62% |
| B：启用真实规则 | 第 2–4 轮 | 44,544 / 45,120 | 98.72% |
| B：修改规则后恢复 | 修改后第 2–3 轮 | 30,208 / 30,596 | 98.73% |

- 关闭/启用两组首次请求均实际返回 0 缓存读取；包含首次请求的总命中率分别是 74.17% / 74.22%。不能把这两个冷启动占比较大的数字误当作稳定态表现。
- 修改规则当轮为 **4,480 / 15,193 = 29.49%**；后两轮为 **98.96% / 98.51%**。这与“前缀改变后重新预热，未改变的更早前缀仍可能复用”的语义一致，但单次变更观测不能排除上游其他因素。
- 0.11 个百分点的组间差异不构成“加规则提高缓存”的证据；本实验样本少，目的是发现明显失稳，而不是统计显著性或网关 SLA。
- 稳定规则每次重新发现/渲染均未触发快照重建，系统提示词 SHA-256 保持不变；真正修改正文只触发一次刷新，随后再次稳定。
- 真实规则增加 **2,666 输入 token**：首轮 12,234 → 14,900，约 +21.79%。缓存命中并不意味着这些 token 免费；实际价格、账单未测量。

## 2. 为什么没有直接跑旧测试

原有 `tests/prompt_cache_real_llm_tests.rs::main_agent_prompt_and_tools()` 给 `SystemPromptSnapshot::new` 传入空的 `user_instructions`。`models_toml_model_growing_prefix_cache_marathon` 则用重复生成的 `stable_prefix()`，适合测网关缓存，但不能证明真实规则加载机制是否稳定。

本次接入 `tests/support/custom_rules_cache_probe.rs`，复用原测试的模型配置/凭据加载、临时 HOME、请求构造及流式 usage 采集，只提取原有 `main_agent_context()` 供新旧用例共用；未复制整个约 2,900 行测试文件。

### 测试用例清单

- `tests/support/custom_rules_cache_probe.rs::rendered_rules_keep_snapshot_stable_until_body_changes`：本地确定性用例。覆盖相同内容重写/重新渲染不改快照、正文变更刷新、变更后稳定、删除规则章节刷新。
- `tests/support/custom_rules_cache_probe.rs::project_rules_cache_ab`：默认 `#[ignore]`，手动运行，固定 11 次付费请求。覆盖关闭/开启真实规则的连续请求，以及临时规则正文改变后的重建与恢复。
- 复用 `src/core/llm/tests/system_prompt_test.rs::user_instructions_invalidate_snapshot_without_leaking_into_default_builders`：验证原有快照和默认构建器契约。

## 3. 实验控制与证据边界

- 模型选自当前配置默认项 `deepseek-v4-flash`，协议 `openai`，使用 `models.toml` 对应条目及已有凭据；未打印密钥。
- 规则：`.cursor/rules/engineering-standards.mdc`、`.cursor/rules/no-rm-rf.mdc`、`.cursor/rules/prior-art-before-architecture.mdc`；渲染章节 **6,796 字符**，系统提示词 **17,351 → 24,149 字符**。
- 通过生产 `discover/read_body/render_rules` 读取并在临时目录重建规则；断言其渲染结果与项目原规则逐字节相同。规则变更只发生于临时副本的第一条规则末尾。
- 使用生产 `SystemPromptSnapshot` 和 provider adapter；两组共用 **20 个内置工具定义**。没有加载 Skills/MCP，也没有运行完整 CLI/Serve 会话、工具执行循环、上下文压缩或 `/reload` E2E。
- A/B 历史采用相同、确定性、只追加的合成 `config_get` 工具交换，而非模型随机输出；runtime tail 的内容和角色固定。两组每轮交换先后顺序，减少时间偏差。
- 每组有固定的实验前缀标记与路由 key，减少互相预热；同组后续请求保持不变。工具前缀仍可能被共享，不能仅凭新 key 保证第一次请求完全冷；本次首次请求恰好都返回 0。
- 请求间隔 8 秒，provider retry_count=0，每次完整流式请求外层限时 120 秒。缺凭据、未加载规则、缺 usage/cache_read 字段或 token 口径异常会失败，不会伪装成跳过成功。
- 输出要求短答，非 Responses 请求给出 64-token 参数，但返回 usage 有一轮 completion_tokens=122；这不是服务端严格输出上限的证明。本次总输出实际 **298 token**。
- 模型有一轮返回了 1 个工具调用，已计入记录，**没有执行**，也没有用于推进两组历史。测试不依赖模型是否遵循短答要求，只采集真实 usage。
- `TokenUsage::prompt_tokens` 已包含缓存读取/写入，不能再次相加。公式为 `Σcache_read_tokens / Σprompt_tokens`，不是请求成功率，也不是节省费用比例。
- provider 没返回 cache_write 字段，原始值为 `null`，不解释为写缓存为零。
- 测试通过表示实验完整执行并获取数据，不代表满足预设的命中阈值。第 1–6 节记录 DeepSeek 首轮；第 7 节追加 OpenAI Responses 双网关结果。Anthropic 尚未测试，不能外推所有模型。

## 4. 逐轮原始计数

| 场景 | 轮次 | 输入 token | 缓存读取 token | 命中率 | 输出 token |
| :--- | ---: | ---: | ---: | ---: | ---: |
| 关闭规则 | 1 | 12,234 | 0 | 0.00% | 2 |
| 关闭规则 | 2 | 12,304 | 12,160 | 98.83% | 52 |
| 关闭规则 | 3 | 12,374 | 12,160 | 98.27% | 2 |
| 关闭规则 | 4 | 12,444 | 12,288 | 98.75% | 31 |
| 启用规则 | 1 | 14,900 | 0 | 0.00% | 2 |
| 启用规则 | 2 | 14,970 | 14,720 | 98.33% | 122 |
| 启用规则 | 3 | 15,040 | 14,848 | 98.72% | 2 |
| 启用规则 | 4 | 15,110 | 14,976 | 99.11% | 42 |
| 规则改变后 | 1 | 15,193 | 4,480 | 29.49% | 2 |
| 规则改变后 | 2 | 15,263 | 15,104 | 98.96% | 39 |
| 规则改变后 | 3 | 15,333 | 15,104 | 98.51% | 2 |

总计：11 次请求，输入 **155,165 token**，缓存读取 **115,840 token**，输出 **298 token**；真实探针运行 **92.21 秒**，不含编译。

系统提示词哈希（不含额外固定实验标记）：

| 场景 | SHA-256 |
| :--- | :--- |
| 关闭规则 | `400b0381cd252eb2667d65f60f3ec73d401b34c185a860fe43e26db50183f164` |
| 启用规则 | `277f2ff31ebd3d65d350722f9de7a2b3ff2d531ee18d696cfba75ae1d8bbb71a` |
| 规则改变后 | `e3149e8da2df09c1fd8fddf7aa220848b4d60f9a7ea278b0ae93754a8536b8fd` |

规则章节原始 SHA-256：`9c1cdbd018f4f272078738d0287fa93119021fc38701c8626dc40f278e9d3123`。

## 5. 复现命令与检查记录

在仓库的 `tomcat/` 目录执行：

```sh
# 本地确定性校验，不调用模型
cargo test --test prompt_cache_real_llm_tests \
  custom_rules_cache_probe::rendered_rules_keep_snapshot_stable_until_body_changes \
  -- --exact --nocapture

# 只运行这一条 ignored 探针；会调用真实模型，不要无过滤运行全部 ignored
TOMCAT_E2E_CACHE_PROBE_MODEL=deepseek-v4-flash \
  cargo test --test prompt_cache_real_llm_tests \
  custom_rules_cache_probe::project_rules_cache_ab \
  -- --ignored --exact --nocapture
```

| 检查 | 结果 / 任务 |
| :--- | :--- |
| 新增本地用例 | 1 passed，`1791081269369-u6kqsb`；后续仅格式化该用例，新增原规则副本一致性断言由真实用例执行覆盖 |
| 原有系统快照用例 | 1 passed，`1791081572928-idssec` |
| 真实探针 | 1 passed，0 ignored，11 次实际 usage，`1791081549874-inwo3i` |
| 逐轮数据重算 | 11 行、各阶段唯一哈希及加权计数复核通过，`1791081962794-msypt3` |
| 格式 | 初次 rustfmt check 因新文件格式失败；只格式化新文件后通过，`1791081517516-aqekkf` |
| 严格 Clippy | `cargo clippy --test prompt_cache_real_llm_tests --no-deps -- -D warnings` exit 101，`1791081930793-y0n6ay`；先被未改动生产文件 `src/api/chat/commands/parse.rs:91` 的 `clippy::large_enum_variant` 阻塞，不能称为通过 |
| 排除已知阻塞后的定向 Clippy | 同一命令仅追加 `-A clippy::large_enum_variant`，exit 0，`1791082060025-qdv476`；未修改生产文件，严格原命令的失败记录仍保留 |
| 最终格式与空白 | rustfmt check / git diff --check 通过，`1791082171860-2y99by`；生产 `tomcat/src/` 与 HEAD 无差异 |

原始模型数据日志：`/Users/yankeben/.tomcat/agents/main/tool-results/bash-1791081549874-inwo3i.log`。工具日志可能在一行中插入多个 `STDERR: ` 前缀，解析 JSON 时需先去除这个工具展示前缀；不修改原日志。

## 6. 是否需要改生产架构

**DeepSeek 首轮证据不支持为自定义规则引入额外缓存、迁移章节或增加新的失效协议。** 规则未改变时，现有字节稳定的系统前缀已经获得连续命中；规则真正改变后的一次重新预热是预期成本。GPT-5.6 双网关补测（第 7 节）出现较低比例及一次零命中，仍不能直接定位为章节位置问题；先查明实际 wire/usage/上游路由差异，再决定是否改生产实现。

## 7. GPT-5.6 双网关补测

### 7.1 测量对象与结果

按用户指定，读取 `~/.tomcat/models.toml` 的实际配置，选择两家都有的同名模型，而不是将 sol/luna/terra 混在一起：

| 配置 ID | 协议 | 配置 endpoint |
| :--- | :--- | :--- |
| `fcodex/gpt-5.6-terra` | `openai-responses` | `https://fcodex.top/` |
| `idatatlas/gpt-5.6-terra` | `openai-responses` | `https://sub2api.idatatlas.com/` |

原样复用专项探针，未修改测试代码或配置；两家各 11 请求、不同进程独立临时 HOME 并行运行，每家内部仍串行、间隔 8 秒。相同的三条规则、6,796 字符及 20 个工具定义，关闭/启用规则时的系统哈希均与 DeepSeek 首轮相同。规则改变后的哈希在各自组内保持不变，但两家之间不同，因为临时改动包含各自实验 nonce。

```text
                         fcodex             idatatlas
关闭规则，第 2–4 轮       93.16%             94.91%
启用规则，第 2–4 轮       90.25%             92.12%
改规则当轮               38.01%             43.65%
改规则后下一轮            0.00%             98.78%
再下一轮                 98.37%             98.37%
```

| 场景 | fcodex：缓存读取 / 输入 token | fcodex 加权命中率 | idatatlas：缓存读取 / 输入 token | idatatlas 加权命中率 |
| :--- | ---: | ---: | ---: | ---: |
| 关闭规则，第 2–4 轮 | 31,032 / 33,312 | 93.16% | 28,160 / 29,670 | 94.91% |
| 启用规则，第 2–4 轮 | 46,986 / 52,061 | 90.25% | 35,328 / 38,349 | 92.12% |
| 改规则后第 2–3 轮 | 12,800 / 34,781 | 36.80% | 25,600 / 25,970 | 98.58% |

两家关闭/启用规则的首次请求均为 0 命中。包含各阶段第一轮的总比例分别为：fcodex 关闭 62.38%、启用 64.05%、改变后 37.24%；idatatlas 关闭 71.38%、启用 69.24%、改变后 80.34%。首轮占比会明显影响平均值。

**这次不能把 DeepSeek 的约 98.7% 直接推广到 GPT-5.6。** 后续三轮开启规则的比例比关闭规则分别低约 2.90 / 2.79 个百分点；小样本不足以证明这是规则机制本身的因果影响，但应如实报告差异。两家 B 组第 2–4 轮都持续有缓存读取，不过读取长度分别固定为 15,662 / 11,776，未观察到该阶段命中长度随小幅历史追加增长，不能称为整段历史完全复用。

### 7.2 逐轮计数

| 场景 | 轮次 | fcodex 输入 | fcodex 缓存读取 | fcodex 命中率 | idatatlas 输入 | idatatlas 缓存读取 | idatatlas 命中率 |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 关闭规则 | 1 | 16,434 | 0 | 0.00% | 9,782 | 0 | 0.00% |
| 关闭规则 | 2 | 13,478 | 11,576 | 85.89% | 9,836 | 8,704 | 88.49% |
| 关闭规则 | 3 | 9,890 | 9,728 | 98.36% | 9,890 | 9,728 | 98.36% |
| 关闭规则 | 4 | 9,944 | 9,728 | 97.83% | 9,944 | 9,728 | 97.83% |
| 启用规则 | 1 | 21,294 | 0 | 0.00% | 12,675 | 0 | 0.00% |
| 启用规则 | 2 | 17,263 | 15,662 | 90.73% | 12,729 | 11,776 | 92.51% |
| 启用规则 | 3 | 17,354 | 15,662 | 90.25% | 12,783 | 11,776 | 92.12% |
| 启用规则 | 4 | 17,444 | 15,662 | 89.78% | 12,837 | 11,776 | 91.73% |
| 规则改变后 | 1 | 19,708 | 7,491 | 38.01% | 12,904 | 5,632 | 43.65% |
| 规则改变后 | 2 | 21,769 | 0 | 0.00% | 12,958 | 12,800 | 98.78% |
| 规则改变后 | 3 | 13,012 | 12,800 | 98.37% | 13,012 | 12,800 | 98.37% |

fcodex：累计报告输入 177,590、缓存读取 98,309、输出 320 token，探针耗时 134.57 秒。idatatlas：累计报告输入 129,350、缓存读取 94,720、输出 178 token，探针耗时 117.10 秒。两次用例均完整运行 11 请求，1 passed / 0 ignored；这是诊断执行通过，不是缓存稳定性 SLA 通过。fcodex 末轮返回一个工具调用，未执行；idatatlas 没有返回工具调用。

### 7.3 fcodex 的异常与归因边界

**现象：** 系统提示词在同阶段哈希相同，合成历史只追加，fcodex 返回的输入计数却有大幅倒退：

- 关闭规则：16,434 → 13,478 → 9,890 → 9,944。
- 启用规则：21,294 → 17,263 → 17,354 → 17,444。
- 改规则之后：19,708 → 21,769 → 13,012，中间一轮缓存读取为 0。

同阶段 idatatlas 的输入计数每轮正常增加 54，且变化后的下一轮恢复高比例命中。**本次观察中 idatatlas 的计数与变更后恢复更连贯；fcodex 出现额外零命中和计数跳变，不能保证持续命中。** 末轮回到 98.37% 不等于已证明重新稳定。

这些数据来自 Tomcat adapter 解析后的 provider usage。系统哈希稳定并不等于已经审计了最终 HTTP body。尚未捕获服务端收到的完整请求、原始 usage、实际后端模型和路由身份，因此不能断言“必然是网关多上游”“必然是计费倍率”“必然是 Tomcat 丢缓存”，也不据此声称两家实际费用相同或不同。若继续定位，应先对照出站 payload 与原始 usage，而不是重复跑到全绿或直接改章节位置。

### 7.4 复现与证据

在 `tomcat/` 下分别执行（每条各 11 次真实请求）：

```sh
TOMCAT_E2E_CACHE_PROBE_MODEL=fcodex/gpt-5.6-terra \
  cargo test --test prompt_cache_real_llm_tests \
  custom_rules_cache_probe::project_rules_cache_ab \
  -- --ignored --exact --nocapture

TOMCAT_E2E_CACHE_PROBE_MODEL=idatatlas/gpt-5.6-terra \
  cargo test --test prompt_cache_real_llm_tests \
  custom_rules_cache_probe::project_rules_cache_ab \
  -- --ignored --exact --nocapture
```

- fcodex：任务 `1791082726308-a9n8o3`，exit 0；原始日志 `/Users/yankeben/.tomcat/agents/main/tool-results/bash-1791082726308-a9n8o3.log`。
- idatatlas：任务 `1791082726351-63wbm9`，exit 0；原始日志 `/Users/yankeben/.tomcat/agents/main/tool-results/bash-1791082726351-63wbm9.log`。
- 数据重算：任务 `1791082898955-q41ms0`，exit 0；确认各 11 行、6 组汇总，各阶段单一哈希，所有加权计数与探针输出一致；明确列出了计数倒退。
- 沿用第 5 节仍有效的本地契约和定向代码检查结果，保留原严格 Clippy 阻塞记录；本轮没有改动探针、生产代码或真实配置，只补充报告。
