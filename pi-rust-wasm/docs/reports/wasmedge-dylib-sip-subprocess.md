# WasmEdge 动态库 × macOS SIP × 子进程：问题分析与解决方案

> 状态：**待实施**（方案已验证，等待评审后择一落地）

---

## 1. 问题现象

| 场景 | 结果 |
|------|------|
| 终端直接运行 `pi chat` | 正常启动 ✓ |
| `pi chat` 内 LLM 调用 `execute_bash` 执行 `pi --help` | `dyld: Library not loaded: @rpath/libwasmedge.0.dylib` ✗ |

同一台机器、同一个二进制、WasmEdge 已安装——主进程能跑，子进程崩溃。

---

## 2. 根因

### 2.1 动态链接依赖链

```
pi 二进制
  → 依赖 @rpath/libwasmedge.0.dylib（动态链接，默认 feature 未启用 standalone）
  → 运行时由 macOS dyld 负责加载
  → dyld 搜索顺序：二进制内嵌 LC_RPATH → DYLD_LIBRARY_PATH 环境变量 → 系统默认路径
```

当前 `pi` 二进制**无 LC_RPATH**（可用 `otool -l pi | grep -A2 LC_RPATH` 验证为空），完全依赖 `DYLD_LIBRARY_PATH` 环境变量。

### 2.2 macOS SIP 剥离 DYLD_* 环境变量

macOS **系统完整性保护（SIP）** 对位于受保护路径（`/bin/`、`/usr/bin/`、`/System/` 等）的可执行文件，在 `execve` 时**自动从环境中剥离所有 `DYLD_*` 变量**。

`execute_bash` 使用 `Command::new("sh")`，即 `/bin/sh`——SIP 保护路径。

### 2.3 完整调用链

```
用户 bash shell
  ┌ .bash_profile → source ~/.wasmedge/env
  │   → export DYLD_LIBRARY_PATH=$HOME/.wasmedge/lib
  │
  ├─ 直接运行 pi chat
  │    → pi 继承 DYLD_LIBRARY_PATH
  │    → dyld 从该路径找到 libwasmedge.0.dylib
  │    → 正常启动 ✓
  │
  └─ pi 内部 execute_bash:
       → Command::new("sh").arg("-c").arg("pi --help")
       → Rust tokio 调用 execve("/bin/sh", ...)
       → macOS 内核检测到 /bin/sh 在 SIP 保护路径
       → 内核剥离 DYLD_LIBRARY_PATH（及所有 DYLD_*）
       → /bin/sh 启动，环境中无 DYLD_LIBRARY_PATH
       → sh 执行 pi --help → 新 pi 进程启动
       → dyld 搜索库：LC_RPATH（无）→ DYLD_LIBRARY_PATH（空）→ 默认路径（无）
       → Library not loaded ✗
```

### 2.4 为什么主进程不受影响

用户的 `pi` 二进制位于 `~/.cargo/bin/` 或项目 `target/` 下——**不在 SIP 保护路径**。从用户 shell 直接启动 `pi` 时，macOS 不剥离环境变量。

**SIP 只在 exec 受保护二进制那一刻剥离，不影响非受保护二进制之间的环境继承。**

### 2.5 实测验证

```bash
# 验证 SIP 剥离
DYLD_LIBRARY_PATH="/test" /bin/sh -c 'echo $DYLD_LIBRARY_PATH'
# 输出：空

# 验证 rpath 绕过 SIP
cp target/release/pi /tmp/pi_copy
install_name_tool -add_rpath ~/.wasmedge/lib /tmp/pi_copy
/bin/sh -c '/tmp/pi_copy --help'
# 输出：正常 ✓（rpath 写在二进制内，不依赖环境变量）
```

---

## 3. 解决方案

### 方案 A：`@executable_path` 相对 rpath + 约定安装目录（推荐分发）

**原理**：`build.rs` 注入 `@executable_path/../lib` 作为 rpath。dyld 在运行时将其解析为**二进制所在目录的相对路径**，不依赖环境变量，不含用户名，跨机器通用。

**实现**：

```rust
// build.rs
println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../lib");
```

**安装目录约定**：

```
~/.pi/
  bin/pi                          ← 二进制
  lib/libwasmedge.0.dylib         ← 动态库
```

| 用户 | 二进制位置 | rpath 解析为 | 结果 |
|------|-----------|-------------|------|
| alice | /Users/alice/.pi/bin/pi | /Users/alice/.pi/lib/ | ✓ |
| bob | /Users/bob/.pi/bin/pi | /Users/bob/.pi/lib/ | ✓ |
| 系统级 | /usr/local/bin/pi | /usr/local/lib/ | ✓ |

**优点**：一行 build.rs，零运行时代码，跨用户分发无需改二进制。

**缺点**：要求库与二进制的相对位置固定。

---

### 方案 B：配置驱动 + `execute_bash` 注入 export（推荐灵活场景）

**原理**：`pi init` 检测或安装 WasmEdge，将库的绝对路径写入配置文件。`execute_bash` 在构造 sh 命令时，从配置读取路径并包装为 `export DYLD_LIBRARY_PATH=...;` 前缀。sh 内部设的环境变量不受 SIP 影响（SIP 只在 exec 入口剥离外部传入的值）。

**流程**：

```
pi init
  ① 释放内嵌的 install-wasmedge.sh 到 assets/
  ② 检测 ~/.wasmedge/lib/libwasmedge.0.dylib 或 /usr/local/lib/libwasmedge.0.dylib
  ③ 已安装 → 取绝对路径；未安装 → 执行脚本安装（免 sudo，装到 ~/.wasmedge/）
  ④ 将路径写入 pi.config.toml：wasmedge_lib_dir = "/Users/X/.wasmedge/lib"
  ⑤ 配置 shell 环境变量（给主进程用）：往 .bashrc/.zshrc 追加 source ~/.wasmedge/env
```

**execute_bash 改动**（约 10 行）：

```rust
// src/core/executor.rs — execute_bash 内
let final_cmd = if let Some(lib_dir) = &self.config.wasmedge_lib_dir {
    format!(
        "export DYLD_LIBRARY_PATH=\"{0}\" LD_LIBRARY_PATH=\"{0}\"; {1}",
        lib_dir, command
    )
} else {
    command.to_string()
};

Command::new(shell)
    .arg(arg)
    .arg(&final_cmd)
    .current_dir(&cwd_path)
    .kill_on_drop(true)
    .output()
    .await
```

**为什么 sh 内部 export 能绕过 SIP**：

```
Command::new("sh") → execve /bin/sh → SIP 剥离外部 DYLD（无所谓）
  → sh 执行脚本：export DYLD_LIBRARY_PATH=/path  ← sh 自己设的，有效
  → sh fork+exec pi（非 SIP 路径）→ macOS 不剥离 → pi 拿到变量 → 找到库 ✓
```

**优点**：库路径灵活，不要求固定相对位置；init 全自动。

**缺点**：需要运行时配置管理；命令包装有边界情况需处理。

---

### 方案 C：`build.rs` 注入绝对 rpath（仅开发自用）

```rust
// build.rs — 检测编译机的 wasmedge 路径并注入
if let Ok(lib_dir) = std::env::var("WASMEDGE_LIB_DIR") {
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir);
} else {
    let home = std::env::var("HOME").unwrap_or_default();
    let default = format!("{}/.wasmedge/lib", home);
    if std::path::Path::new(&default).exists() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", default);
    }
}
```

**优点**：最简单，开发者编译即用。

**缺点**：路径含编译机用户名，不可分发。

---

### 方案 D：静态链接（发布分发最简）

```bash
cargo build --release --features standalone
```

直接把 WasmEdge 静态链接进二进制，无动态库依赖。

**优点**：零配置，单文件分发。

**缺点**：二进制体积增大；不适合需要动态加载插件的场景。

---

## 4. 方案对比

| | A: @executable_path | B: 配置+注入 | C: 绝对rpath | D: 静态链接 |
|---|---|---|---|---|
| 主进程 | ✓ | ✓（靠 shell env） | ✓ | ✓ |
| 子进程（SIP） | ✓ | ✓ | ✓ | ✓ |
| 跨用户分发 | ✓ | ✓ | ✗ | ✓ |
| 免 sudo | 取决于安装位置 | ✓ | ✓ | ✓ |
| 运行时代码改动 | 无 | ~10 行 | 无 | 无 |
| 编译时改动 | build.rs 1 行 | build.rs + config | build.rs ~10 行 | Cargo.toml feature |
| 库位置灵活 | 须固定相对路径 | 任意 | 固定编译机路径 | N/A |

---

## 5. 推荐组合

- **开发期**：方案 C（build.rs 绝对 rpath）— 零配置，编译即用
- **分发到用户**：方案 A（@executable_path）或 方案 D（静态链接）
- **灵活部署 + 自动初始化**：方案 B（配置驱动 + execute_bash 注入）

方案之间不互斥，可叠加。例如：**A + B** = 固定位置有 rpath 兜底，自定义位置靠配置注入。

---

## 6. 相关文件

| 文件 | 说明 |
|------|------|
| `build.rs` | 编译脚本，注入 rpath |
| `src/core/executor.rs` — `execute_bash` | 子进程启动，注入 DYLD export |
| `scripts/install-wasmedge.sh` | WasmEdge 安装脚本 |
| `Cargo.toml` — `features.standalone` | 静态链接 feature |
| `src/infra/config.rs` | 配置管理，`wasmedge_lib_dir` 字段 |
| `docs/reports/wasmedge-standalone-build-and-linking.md` | 静态链接相关的已有报告 |
