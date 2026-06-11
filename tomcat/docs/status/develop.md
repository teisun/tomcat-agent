| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-06-11 23:39 +0800 | ACTIVE | develop | — |

### 2026-06-11 | fix(install): 改用 release metadata 下载资产

- **动机**：本机执行 `curl .../install.sh | bash` 时，直连 `github.com/.../releases/download/...` 获取 `SHA256SUMS` / tar.gz 出现 `HTTP2 framing layer` 与超时，导致一键安装不稳定。
- **实现**：`install.sh` 改为先读取 GitHub release 元数据，再解析目标 asset 的 API 下载地址与 `sha256` digest；下载阶段优先走 `python3 + urllib`，失败后回退到 `curl --http1.1 --retry-all-errors`，不再依赖单独下载 `SHA256SUMS` 直链。
- **验证**：已在本机用临时 `HOME` 完整执行 `bash tomcat/scripts/install.sh -y -v v0.1.4`，成功安装并输出 `tomcat 0.1.4`。

