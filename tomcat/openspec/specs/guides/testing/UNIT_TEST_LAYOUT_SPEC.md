# 单元测试文件组织规范 (Unit Test Layout Spec)

Rust 社区习惯把模块测试写在同文件底部，但这会使**业务逻辑与测试代码混杂在同一文件中**，严重膨胀行数并干扰对主逻辑的阅读。

本规范是单元测试**物理位置与模块挂载**的**单一权威来源**。[UNIT_TEST_SPEC.md](UNIT_TEST_SPEC.md)（编写规范：mock、覆盖率、命名、断言等）与 [PLAN_SPEC.md](../../../../agents/plan/PLAN_SPEC.md) 中涉及测试布局的计划描述，均以本节为准；如出现表述冲突以本节为权威。行数度量与拆分动机见 [RUST_FILE_LINES_SPEC.md](../coding/RUST_FILE_LINES_SPEC.md)。

---

## 核心原则

> 测试**物理位置**一律落「父目录模块」的 `tests/` 子目录下，与同级业务源文件并排；**禁止**单文件 `foo.rs` 旁开 `foo/tests/` 空壳目录。**模块挂载**默认走父目录 `tests/mod.rs`；当且仅当测试需要访问被测模块的私有项时，走第 9 条的 `#[cfg(test)] #[path]` 挂载——**禁止**为测试在业务源文件提升可见性（`pub(super)` / `pub(crate)`）。

---

## 1. 目录结构与文件命名

单元测试**必须**放在**模块所在目录**（含 `mod.rs` 的目录，或 crate root，下称「父目录模块」）的 `tests/` 子目录中，采用：

```text
<dir>/tests/mod.rs
<dir>/tests/<stem>_test.rs              # 与被测业务子文件一一对应（默认）
<dir>/tests/<stem>_<topic>_test.rs       # 按主题再拆（可选）
```

其中 `<stem>` 为被测业务源文件的主名（不含 `.rs`）：

- `foo/bar.rs` → `foo/tests/bar_test.rs`
- `foo.rs`（单文件模块）→ `tests/foo_test.rs`

**禁止**在 `tests/` 内使用与业务源文件同名的 `bar.rs`（易与 `mod bar;` 语义混淆）；测试文件**必须**带 `_test` 后缀。

跨用例共享的 mock / helper 可用 `tests/mocks.rs`；多文件聚合冒烟可用 `tests/suite_test.rs`（不对应单一业务源文件时）。

---

## 2. 禁止单文件 + 同名空壳目录

不允许在 `foo.rs` 旁建 `foo/tests/` 目录承载其测试。`foo.rs` 自身的测试**必须**落到其所在父目录模块的 `tests/foo_test.rs`；体量较大、按主题拆分更易读时，继续拆为 `tests/foo_<topic>_test.rs`（如 `tests/foo_header_test.rs` / `tests/foo_lookup_test.rs`）。

---

## 3. 模块入口声明

父目录模块的入口文件（`mod.rs` 或 crate root）中，唯一允许的测试模块入口是 `#[cfg(test)] mod tests;`（单行声明，解析到 `<dir>/tests/mod.rs`）。`tests/mod.rs` 中声明测试子模块时使用 `mod <stem>_test;`（与文件名一致，不含 `.rs`）。被测业务源文件**不再**声明自己的 `mod tests;`（第 9 条的 `#[cfg(test)] #[path] mod tests;` 是为「测私有项」明确开放的唯一例外）。

---

## 4. 小模块最低结构

小模块即使测试很少，也至少落地 `<dir>/tests/mod.rs` + 至少一个 `<dir>/tests/<stem>_test.rs`，**不允许**以单文件 `tests.rs` 平铺占位。

---

## 5. 大文件拆分与共享 helper

单个测试文件体量过大、评审或导航吃力时，建议按主题再拆 `tests/<stem>_<topic>_test.rs`；跨用例共享的 mock / helper 放 `tests/mocks.rs` 或 `tests/mod.rs` 统一复用。

---

## 6. 禁止逃避通道

以下写法均不允许 ——

- `foo/tests.rs`（与 `foo.rs` 平级单文件）；
- `foo_tests.rs`（与 `foo.rs` **同级**、父级目录平铺；**不是** `tests/foo_test.rs`）；
- `foo/tests/`（在单文件 `foo.rs` 旁开同名空壳目录）。

测试归属**始终**是「父目录模块」，由 `<dir>/tests/` 统一承载；`tests/<stem>_test.rs` 为**推荐且默认**的合法命名。

---

## 7. 禁止内联测试块

业务源文件中仍禁止内联 `#[cfg(test)] mod tests { ... }` 块；仅允许两种声明形式：

- 父目录模块入口文件中的 `#[cfg(test)] mod tests;`（默认，挂到父目录 `tests/mod.rs`）；
- 被测业务源文件末尾的 `#[cfg(test)] #[path = "tests/<stem>_test.rs"] mod tests;`（仅当需要测私有项时使用，详见第 9 条）。

必要时仍可在类型/函数上加 `#[cfg(test)]` 暴露测试 helper。

---

## 8. 与集成测试的边界

集成测试放项目 `tests/` 顶层目录，与本规范无冲突。

---

## 9. 测私有项：`#[path]` 挂载

**测私有项必须走 `#[path]` 挂载，禁止为测试放宽可见性。** 当测试用例需要访问被测模块的私有 `fn` / 私有字段 / 私有 `const` 时：

- 测试文件**物理位置**仍统一在父目录 `<dir>/tests/<stem>_test.rs` 或 `<dir>/tests/<stem>_<topic>_test.rs`（与第 1 条一致，不另立位置规则）；
- 在被测源文件 `<file>.rs` 末尾声明**唯一一行**：

  ```rust
  #[cfg(test)]
  #[path = "tests/<stem>_test.rs"]
  mod tests;
  ```

  `#[path]` 的相对基准是 `<file>.rs` **所在目录**（即父目录），自动命中已有测试文件，物理路径无需调整；
- 父目录 `<dir>/tests/mod.rs` **不再**声明 `mod <stem>_test;`（同一文件被两个 `mod` 引用，rustc 会报「file ... is included multiple times」）；
- 此时测试编译期作为 `<file>` 的子模块，`super::*` 即被测模块自身，私有项天然可见，测试文件直接 `use super::*;` 或 `use super::<item>;` 引入即可，**禁止**在被测源文件出现 `pub(super)` / `pub(crate)` 等仅为测试服务的可见性提升。
- **二选一**：仅测公共 API 的文件走第 1 / 第 3 条（默认，标准父目录 `mod.rs` 声明）；需要测私有项的文件走本条 `#[path]` 挂载——两者不能并存于同一被测文件。
- 巡检：`rg "#\[cfg\(test\)\]\s*\n\s*#\[path" tomcat/src` 可定位所有 `#[path]` 挂载点，便于审计与扩散控制。

**示例**：

- 公共 API 测试（默认）—— `foo/bar.rs` 的测试在 `foo/tests/bar_test.rs`，由 `foo/tests/mod.rs` 声明 `mod bar_test;`。
- 私有项测试（例外）—— `foo/baz.rs` 的测试也在 `foo/tests/baz_test.rs`，但由 `baz.rs` 末尾的 `#[cfg(test)] #[path = "tests/baz_test.rs"] mod tests;` 挂载，`foo/tests/mod.rs` 中**不**声明 `mod baz_test;`。

---

## 收益

业务文件行数即为有效逻辑行数，度量不再需要区分测试与非测试；业务文件更精简，阅读与评审体验更好；测试与同级源文件并排，`_test` 后缀在 `tests/` 目录内一眼可辨。

更细的测试编写规范（mock 策略、覆盖率、命名、断言等）见 [UNIT_TEST_SPEC.md](UNIT_TEST_SPEC.md)。

---

## 参考

- [UNIT_TEST_SPEC.md](UNIT_TEST_SPEC.md) — 单元测试编写规范
- [RUST_FILE_LINES_SPEC.md](../coding/RUST_FILE_LINES_SPEC.md) — 业务文件行数区间与拆分策略
- [INTEGRATION_TEST_SPEC.md](INTEGRATION_TEST_SPEC.md) — 集成测试（项目顶层 `tests/`）
