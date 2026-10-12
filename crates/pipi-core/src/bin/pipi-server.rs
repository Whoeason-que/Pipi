//! Web/远程宿主入口（薄壳：实现见 `pipi_core::server`）。

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pipi_core::server::run().await
}
