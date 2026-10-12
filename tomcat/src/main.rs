//! tomcat 二进制入口：解析 CLI 并派发到子命令。

fn main() {
    if let Err(e) = tomcat::run_cli() {
        eprintln!("{}: {}", tomcat::infra::tr("cli.error", &[]), e);
        std::process::exit(1);
    }
}
