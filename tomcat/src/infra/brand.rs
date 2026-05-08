//! 品牌与命名常量的单一事实源。
//!
//! 说明：
//! - `INTERNAL_STABLE_ID`：内部长期稳定标识；
//! - `BRAND_ID`：对外品牌标识；
//! - 当前两者均为 `tomcat`（按重命名计划约定）。

/// 内部长期稳定 ID。
pub const INTERNAL_STABLE_ID: &str = "tomcat";

/// 外部品牌 ID。
pub const BRAND_ID: &str = "tomcat";

/// CLI 可执行名。
pub const CLI_NAME: &str = "tomcat";

/// 用户可见产品名（首字母大写）。
pub const PRODUCT_NAME: &str = "Tomcat";

/// 配置环境变量前缀（`TOMCAT__*`）。
pub const ENV_PREFIX: &str = "TOMCAT";

/// 默认数据根目录。
pub const DEFAULT_WORK_DIR: &str = "~/.tomcat/";

/// 默认配置文件名。
pub const DEFAULT_CONFIG_FILENAME: &str = "tomcat.config.toml";

/// 默认配置文件绝对逻辑路径（含 `~`）。
pub const DEFAULT_CONFIG_PATH: &str = "~/.tomcat/tomcat.config.toml";

/// QuickJS modules 路径覆盖环境变量。
pub const QUICKJS_MODULES_PATH_ENV: &str = "TOMCAT_QUICKJS_MODULES_PATH";
