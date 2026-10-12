//! openside command-line client.

#[tokio::main]
async fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(openside_cli::run_process(std::env::args_os()).await)
}
