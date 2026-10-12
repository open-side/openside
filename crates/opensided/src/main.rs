//! Command-line entrypoint for the local daemon.

#[tokio::main]
async fn main() {
    if let Err(error) = opensided::main().await {
        opensided::diag!("opensided: {error}");
        std::process::exit(1);
    }
}
