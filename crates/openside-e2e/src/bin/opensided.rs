//! Real daemon entrypoint for process tests.

#[tokio::main]
async fn main() {
    if let Err(error) = opensided::main().await {
        eprintln!("e2e-opensided: {error}");
        std::process::exit(1);
    }
}
