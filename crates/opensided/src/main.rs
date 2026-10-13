//! Command-line entrypoint for the local daemon.

#[allow(unsafe_code)]
fn main() {
    // No runtime or other thread exists yet.
    unsafe {
        opensided::capture_restart_marker();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("runtime: {error}"));
    if let Err(error) = runtime.block_on(opensided::main()) {
        opensided::diag!("opensided: {error}");
        std::process::exit(1);
    }
}
