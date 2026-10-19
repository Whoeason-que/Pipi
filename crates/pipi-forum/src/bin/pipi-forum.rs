//! pipi-forum CLI 入口（薄壳：参数分发在 `pipi_forum::cli`）。

fn main() -> std::process::ExitCode {
    pipi_forum::cli::run()
}
