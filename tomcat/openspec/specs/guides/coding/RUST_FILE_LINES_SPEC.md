# Rust 源文件行数与模块拆分 (Rust File Lines Spec)

Rust 编译器对单文件行数没有硬性限制。本规范从**可维护性**、**代码评审体验**以及 **IDE / rust-analyzer 响应速度**出发，给出工程上普遍认可的区间与拆分策略。

**性质**：指导性建议，**不作为** CI 门禁（除非项目另行约定自动化检查）。与具体模块组织方式仍以 [Codeing&Architecture_Spec.md](Codeing&Architecture_Spec.md) 为准。

---

## 度量口径

以下各区间行数均指**非测试业务代码**（不含 `#[cfg(test)]` 块及独立测试文件）。由于单元测试必须放在独立文件中（见 [UNIT_TEST_LAYOUT_SPEC.md](../testing/UNIT_TEST_LAYOUT_SPEC.md)），业务文件行数即为其实际行数。

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

单元测试不得与业务代码混写在同一源文件中。目录布局、模块挂载、`#[path]` 私有项测试等规则见 **[UNIT_TEST_LAYOUT_SPEC.md](../testing/UNIT_TEST_LAYOUT_SPEC.md)**（单一权威来源）；mock 策略、覆盖率、命名与断言等编写约定见 [UNIT_TEST_SPEC.md](../testing/UNIT_TEST_SPEC.md)。

业务与测试分离的直接收益：本规范各区间行数即为有效逻辑行数，度量不再需要区分测试与非测试。

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
- 单元测试**必须**放在独立文件中（见 [UNIT_TEST_LAYOUT_SPEC.md](../testing/UNIT_TEST_LAYOUT_SPEC.md)），业务文件不内联 `#[cfg(test)]` 块。
- 若在编辑或保存时**频繁**遇到 rust-analyzer 变慢、超时或诊断滞后，**优先排查超大单文件**并拆分，往往比调编辑器配置更有效。
