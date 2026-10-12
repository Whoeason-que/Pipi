//! av CLI 入口（薄壳：参数分发在 `pipi_venv::av::cli`）。

fn main() -> std::process::ExitCode {
    pipi_venv::av::run_cli()
}
