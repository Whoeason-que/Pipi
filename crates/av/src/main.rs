//! av CLI 入口（薄壳：参数分发在 `cli`）。

mod cli;

fn main() -> std::process::ExitCode {
    cli::run()
}
