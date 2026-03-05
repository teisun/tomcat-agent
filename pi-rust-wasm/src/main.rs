//! pi-awsm 二进制入口：解析 CLI 并派发到子命令。

fn main() {
    if let Err(e) = pi_awsm::run_cli() {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}
