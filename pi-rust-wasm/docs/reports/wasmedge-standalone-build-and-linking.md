# WasmEdge `standalone` 特性：下载、解压与动态链接说明

本文记录 `wasmedge-sdk` / `wasmedge-sys` 在启用 **`standalone`** feature 时的构建行为，便于排查「为何 `target/release/` 下没有 tar.gz / dylib」「为何仍是动态链接」等问题。与 [初始化体验与资源内嵌](./init-experience-and-embedded-assets.md) §4.1 互补；**本仓库默认不启用 `standalone`**，需时使用 `cargo build --release --features standalone`。

---

## 1. 结论摘要

| 项目 | 说明 |
|------|------|
| 下载来源 | WasmEdge 官方 **GitHub Releases**，按目标三元组选择预编译压缩包 |
| 本地位置 | `target/<profile>/build/wasmedge-sys-<hash>/out/standalone/`，**不是** `target/release/` 根目录 |
| tar.gz 去向 | 下载并校验后**解压**，归档文件通常不长期保留在产物目录 |
| 链接方式（macOS） | **`dylib` 动态链接**（`cargo:rustc-link-lib=dylib=wasmedge`），非静态 `.a` |
| 与「单文件二进制」 | macOS 预编译包内**无** `libwasmedge.a`；真正静态链接需其他发行物或自编译 |

---

## 2. 下载 URL 与校验

构建脚本根据 **目标平台** 选择归档。以 **macOS x86_64、WasmEdge 0.13.5** 为例，`wasmedge-sys` 的 build 输出（见 `target/release/build/wasmedge-sys-*/output`）中可见：

```text
[wasmedge-sys] building archive url for target macos/x86_64
[wasmedge-sys] using archive Remote {
  url: "https://github.com/WasmEdge/WasmEdge/releases/download/0.13.5/WasmEdge-0.13.5-darwin_x86_64.tar.gz",
  checksum: "b7fdfaf59805951241f47690917b501ddfa06d9b6f7e0262e44e784efe4a7b33"
}
```

- **URL 模式**：`https://github.com/WasmEdge/WasmEdge/releases/download/<版本>/<归档名>.tar.gz`
- **归档名** 随平台变化（如 `darwin_x86_64`、`darwin_arm64`、`manylinux*` 等）
- **SHA-256** 由 crate 内置，用于下载后完整性校验

> 用户问题中的拼写 `drawin` 应为 **`darwin`**。

---

## 3. 解压目录结构

典型解压路径（hash 因构建缓存而异）：

```text
target/release/build/wasmedge-sys-<hash>/out/standalone/WasmEdge-0.13.5-Darwin/
├── include/wasmedge/wasmedge.h
└── lib/
    ├── libwasmedge.0.0.3.dylib   # 实际体积较大的实现库
    ├── libwasmedge.0.dylib       # 常指向版本化库的 symlink
    ├── libwasmedge.dylib
    └── *.tbd                     # Apple 文本式 stub，供链接器解析
```

构建日志中会先尝试 `lib64`，再在 `lib` 下找到库：

```text
[wasmedge-sys] searching for libwasmedge at .../lib64
[wasmedge-sys] searching for libwasmedge at .../lib
[wasmedge-sys] found libwasmedge at .../lib
```

---

## 4. 如何「动态链接」到解压后的库

`wasmedge-sys` 通过 **Cargo build script 指令** 告知 rustc / 链接器：

```text
cargo:rustc-link-search=<...>/WasmEdge-0.13.5-Darwin/lib
cargo:rustc-link-lib=dylib=wasmedge
cargo:rustc-env=LD_LIBRARY_PATH=<...>/WasmEdge-0.13.5-Darwin/lib
```

含义简述：

1. **`rustc-link-search`**：链接阶段在解压目录的 `lib/` 中查找 `libwasmedge*.dylib`。
2. **`rustc-link-lib=dylib=wasmedge`**：与 **`libwasmedge`** 按**动态库**方式链接（install name 常为 `@rpath/libwasmedge.0.dylib` 等，以 `otool -L <二进制>` 为准）。
3. **`rustc-env=LD_LIBRARY_PATH`**：影响部分测试/子进程场景下的运行时库搜索（macOS 上实际加载路径还受 **rpath、DYLD 相关策略** 影响）。

因此：**可执行文件本身体积可以较小**（未把 100MB+ 的 dylib 链进 Mach-O 的「文件内」），但**运行时必须能解析到**对应的 `libwasmedge` 系列动态库。

---

## 5. 与 `pi_wasm` 工程的关系

- **`Cargo.toml`**：`standalone = ["wasmedge-sdk/standalone"]`，**默认 `default = []`**，日常开发使用系统安装的 WasmEdge，缩短编译时间。
- 启用 **`--features standalone`** 时，上述下载与解压由 **`wasmedge-sys`** 在依赖构建阶段完成，**无需**事先执行 `install-wasmedge.sh`（仍可能需网络与磁盘空间）。

---

## 6. 分发与打包注意点

- **仅拷贝 `pi` 二进制** 到另一台未安装 WasmEdge 的机器：若该二进制在构建时动态链接了 standalone 解压出的 dylib，则目标机须能通过 **rpath / 环境变量 / 同目录布局** 找到 `libwasmedge`，否则会加载失败。
- **常见做法**：将 `libwasmedge.0.0.3.dylib`（及必要 symlink）与可执行文件放在约定目录，并设置合适的 **rpath** 或文档中说明 `DYLD_LIBRARY_PATH`（生产环境需注意 Apple 对 `DYLD_*` 的限制）。
- **体积**：未 strip 的 `libwasmedge` 动态库体积可达 **百 MB 级**，与「小体积单文件」目标存在张力；若需极致单文件，需评估 **Linux 静态链接路径** 或 **自编译 WasmEdge** 等方案（超出本文范围）。

---

## 7. 可选环境变量（摘自 build 输出线索）

构建脚本会监听部分环境变量，例如：

- `WASMEDGE_STANDALONE_ARCHIVE` — 使用本地或镜像归档替代默认下载 URL（具体语义以 `wasmedge-sys` 源码为准）。
- `WASMEDGE_STANDALONE_PROXY` — 下载走代理时的配置入口。

排障时可查看：

```text
target/<debug|release>/build/wasmedge-sys-<hash>/output
```

其中包含完整 URL、解压路径与 `rustc-link-*` 行。

---

## 8. 参考

- [初始化体验与资源内嵌](./init-experience-and-embedded-assets.md) — feature gate、默认构建策略
- [WasmEdge Releases](https://github.com/WasmEdge/WasmEdge/releases) — 官方预编译包列表
- 依赖 crate：`wasmedge-sys`（版本随 `wasmedge-sdk` 传递），build 逻辑以 crates.io 发布源码为准

---

*文档版本：与 TASK-06 后可选 `standalone` 的 Cargo 配置一致；若升级 `wasmedge-sdk` 版本，请对照新版本 `wasmedge-sys` 的 `output` 与发行说明复核 URL 与链接类型。*
