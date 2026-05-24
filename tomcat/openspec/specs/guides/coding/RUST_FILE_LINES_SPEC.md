# Rust 源文件行数与模块拆分 (Rust File Lines Spec)

Rust 编译器对单文件行数没有硬性限制。本规范从**可维护性**、**代码评审体验**以及 **IDE / rust-analyzer 响应速度**出发，给出工程上普遍认可的区间与拆分策略。

**性质**：指导性建议，**不作为** CI 门禁（除非项目另行约定自动化检查）。与具体模块组织方式仍以 [Codeing&Architecture_Spec.md](Codeing&Architecture_Spec.md) 为准。

---

## 度量口径

以下各区间行数均指**非测试业务代码**（不含 `#[cfg(test)]` 块及独立测试文件）。由于本规范要求单元测试必须放在独立文件中（见 A 节），业务文件行数即为其实际行数。

---

## L-1. 黄金区间：约 300–500 行

在此区间内，通常能同时满足：

- **职责集中**：一个文件对应相对独立的功能边界，读者容易回答「这个文件在系统里做什么」。
- **心智模型清晰**：无需在数千行之间跳转即可把握主流程与关键类型。
- **评审友好**：Pull Request 中单文件 diff 体量可控，评审者负担较低。

---

## L-2. 预警区间：约 500–1000 行

进入该区间时，建议主动做一次「是否该拆」的自检：

- **职责是否过载**：是否混合了多种概念（例如同一文件既管协议又管持久化又管 UI 适配）？能否按子域划到子模块？
- **`impl` 块是否过长**：单个结构体实现大量 trait、或一个 `impl` 内方法过多时，行数会快速膨胀。可考虑将部分 trait 实现挪到独立子模块或文件中。
- **是否缺少私有子模块**：通过 `mod` 拆分可见性边界，往往比单文件堆叠更易读。

---

## L-3. 建议拆分：约 1000 行以上

超过该量级时，常见问题包括：

- **IDE / 分析器变慢**：rust-analyzer 在超大文件上做类型推导、诊断时延迟更明显（例如日志中可能出现的长时间分析回合）。
- **导航成本上升**：即使有跳转定义，在超长文件中上下滚动、定位逻辑仍低效。

**例外**：由工具生成的源码（如 Protobuf、gRPC、部分状态机或绑定代码）体积大是预期行为，可不按手写代码同一标准苛求；若必须手工维护超大文件，应在评审中说明理由并控制变更频率。

---

## Rust 特有的「行数膨胀」因素

### A. 单元测试（`#[cfg(test)]`）

Rust 社区习惯把模块测试写在同文件底部，但这会使**业务逻辑与测试代码混杂在同一文件中**，严重膨胀行数并干扰对主逻辑的阅读。

**本规范要求**（单元测试组织方式的**单一权威来源**；[UNIT_TEST_SPEC.md](../testing/UNIT_TEST_SPEC.md) §3 / §6、[DEBUG_SPEC.md](../../../../agents/plan/DEBUG_SPEC.md) §9.1、[PLAN_SPEC.md](../../../../agents/plan/PLAN_SPEC.md) §8.1 均以本节为准；如出现表述冲突以本节为权威）：

> **核心原则**：测试**物理位置**一律落「父目录模块」的 `tests/` 子目录下，与同级业务源文件并排；**禁止**单文件 `foo.rs` 旁开 `foo/tests/` 空壳目录。**模块挂载**默认走父目录 `tests/mod.rs`；当且仅当测试需要访问被测模块的私有项时，走第 9 条的 `#[cfg(test)] #[path]` 挂载——**禁止**为测试在业务源文件提升可见性（`pub(super)` / `pub(crate)`）。

1. 单元测试**必须**放在**模块所在目录**（含 `mod.rs` 的目录，或 crate root，下称「父目录模块」）的 `tests/` 子目录中，采用 `<dir>/tests/mod.rs` + `<dir>/tests/<file>[_<topic>].rs` 结构；测试文件主名建议与被测业务子文件同名（例如 `foo/bar.rs` 的测试落到 `foo/tests/bar.rs`）。
2. **禁止单文件 + 同名空壳目录**：不允许在 `foo.rs` 旁建 `foo/tests/` 目录承载其测试。`foo.rs` 自身的测试**必须**落到其所在父目录模块的 `tests/foo.rs`；体量较大、按主题拆分更易读时，继续拆为 `tests/foo_<topic>.rs`（如 `tests/foo_header.rs` / `tests/foo_lookup.rs`）。
3. 父目录模块的入口文件（`mod.rs` 或 crate root）中，唯一允许的测试模块入口是 `#[cfg(test)] mod tests;`（单行声明，解析到 `<dir>/tests/mod.rs`）。被测业务源文件**不再**声明自己的 `mod tests;`（第 9 条的 `#[cfg(test)] #[path] mod tests;` 是为「测私有项」明确开放的唯一例外）。
4. 小模块即使测试很少，也至少落地 `<dir>/tests/mod.rs` + 至少一个 `<dir>/tests/<file>.rs`（topic 建议命名为对应被测子文件名），**不允许**以单文件 `tests.rs` 平铺占位。
5. 单个测试文件体量过大、评审或导航吃力时，建议按主题再拆 `tests/<file>_<topic>.rs`；跨用例共享的 mock / helper 放 `tests/mocks.rs` 或 `tests/mod.rs` 统一复用。
6. **禁止逃避通道**：以下三类写法均不允许 ——
   - `foo/tests.rs`（与 `foo.rs` 平级单文件）；
   - `foo_tests.rs`（父级目录平铺）；
   - `foo/tests/`（在单文件 `foo.rs` 旁开同名空壳目录）。
   测试归属**始终**是「父目录模块」，由 `<dir>/tests/` 统一承载。
7. 业务源文件中仍禁止内联 `#[cfg(test)] mod tests { ... }` 块；仅允许两种声明形式：
   - 父目录模块入口文件中的 `#[cfg(test)] mod tests;`（默认，挂到父目录 `tests/mod.rs`）；
   - 被测业务源文件末尾的 `#[cfg(test)] #[path = "tests/<file>.rs"] mod tests;`（仅当需要测私有项时使用，详见第 9 条）。
   必要时仍可在类型/函数上加 `#[cfg(test)]` 暴露测试 helper。
8. 集成测试放项目 `tests/` 顶层目录，与本条无冲突。
9. **测私有项必须走 `#[path]` 挂载，禁止为测试放宽可见性。** 当测试用例需要访问被测模块的私有 `fn` / 私有字段 / 私有 `const` 时：
   - 测试文件**物理位置**仍统一在父目录 `<dir>/tests/<file>[_<topic>].rs`（与第 1 条一致，不另立位置规则）；
   - 在被测源文件 `<file>.rs` 末尾声明**唯一一行**：

     ```rust
     #[cfg(test)]
     #[path = "tests/<file>.rs"]
     mod tests;
     ```

     `#[path]` 的相对基准是 `<file>.rs` **所在目录**（即父目录），自动命中已有测试文件，物理路径无需调整；
   - 父目录 `<dir>/tests/mod.rs` **不再**声明 `mod <file>;`（同一文件被两个 `mod` 引用，rustc 会报「file ... is included multiple times」）；
   - 此时测试编译期作为 `<file>` 的子模块，`super::*` 即被测模块自身，私有项天然可见，测试文件直接 `use super::*;` 或 `use super::<item>;` 引入即可，**禁止**在被测源文件出现 `pub(super)` / `pub(crate)` 等仅为测试服务的可见性提升。
   - **二选一**：仅测公共 API 的文件走第 1 / 第 3 条（默认，标准父目录 `mod.rs` 声明）；需要测私有项的文件走本条 `#[path]` 挂载——两者不能并存于同一被测文件。
   - 巡检：`rg "#\[cfg\(test\)\]\s*\n\s*#\[path" tomcat/src` 可定位所有 `#[path]` 挂载点，便于审计与扩散控制。

   **示例**：
   - 公共 API 测试（默认）—— `foo/bar.rs` 的测试在 `foo/tests/bar.rs`，由 `foo/tests/mod.rs` 声明 `mod bar;`。
   - 私有项测试（例外）—— `foo/baz.rs` 的测试也在 `foo/tests/baz.rs`，但由 `baz.rs` 末尾的 `#[cfg(test)] #[path = "tests/baz.rs"] mod tests;` 挂载，`foo/tests/mod.rs` 中**不**声明 `mod baz;`。

这样做的收益：业务文件行数即为有效逻辑行数，度量不再需要区分测试与非测试；业务文件更精简，阅读与评审体验更好；测试与同级源文件并排，新增/查找用例的认知成本最低。

更细的测试编写规范（mock 策略、覆盖率、命名、断言等）见 [UNIT_TEST_SPEC.md](../testing/UNIT_TEST_SPEC.md)。

### B. 多个 `impl` 与 trait 实现

同一类型可以在不同模块/文件中分散多个 `impl` 块。

- **技巧**：将核心 API 留在主文件（如 `user.rs`），将 `impl Display for User`、`impl From<Other> for User` 等迁到如 `user/display.rs`、`user/convert.rs` 等子模块，由父模块 `mod` 声明聚合。

### C. 宏与 `derive`

宏展开后的有效工作量可能远大于肉眼所见的行数；大量 `derive` 或复杂过程宏会增加编译器与语言服务后台成本。

- **注意**：避免无必要地堆叠 derive 或宏层数；对热点路径上的类型尤其要克制。

### D. 条件编译（`#[cfg(feature = "...")]`）

与测试类似，大量 feature-gated 代码块（如平台适配、可选后端实现）也会膨胀单文件行数。当同一文件中存在多个 `#[cfg(feature)]` 分支且各分支体量较大时，应将各分支拆为独立子模块，由父模块通过 `#[cfg]` 选择性引入：

```rust
// engine.rs — 按 feature 选择子模块
#[cfg(feature = "backend_a")]
mod backend_a;
#[cfg(feature = "backend_b")]
mod backend_b;
```

---

## 如何拆分：利用模块系统做「细胞分裂」

以下为常见**模式**，目录名可按领域自行命名，不必机械照搬。

1. **目录化**  
   将单文件 `models.rs` 拆分为目录模块：创建 `models/` 目录，保留 `mod.rs` 作为模块入口（声明子模块并 re-export），子文件放入 `models/` 中。这是 Rust 2018+ 的推荐做法（`models.rs` 与 `models/` 子目录并存）。

2. **按职责分文件（示例）**  
   - `models/types.rs`：结构体、枚举、简单常量  
   - `models/impls.rs` 或按领域再拆：业务方法  
   - `models/tests/mod.rs` + `models/tests/*.rs`：对应的单元测试（必须目录化拆分）

3. **trait 实现外提**  
   标准库风格与社区实践均支持「类型定义一处、trait 实现按 trait 或按读者场景分文件」。

简短结构示例（示意，非唯一写法）：

```rust
// models.rs — 门面：对外 re-export 或声明子模块
mod types;
mod service;

pub use types::*;
pub use service::ModelService;
```

---

## 总结

- **优先**将业务代码控制在约 **300–500 行**（黄金区间）；超过 500 行时主动评估是否需拆分。
- 单元测试**必须**放在独立文件中，业务文件不内联 `#[cfg(test)]` 块。
- 若在编辑或保存时**频繁**遇到 rust-analyzer 变慢、超时或诊断滞后，**优先排查超大单文件**并拆分，往往比调编辑器配置更有效。
