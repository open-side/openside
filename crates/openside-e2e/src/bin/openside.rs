//! Real CLI entrypoint for process tests.

#[tokio::main]
async fn main() {
    std::process::exit(i32::from(
        openside_cli::run_process(std::env::args_os()).await,
    ));
}
