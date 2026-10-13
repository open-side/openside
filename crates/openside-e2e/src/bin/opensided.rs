//! Real daemon entrypoint for process tests.

#[allow(unsafe_code)]
fn main() {
    // Capture the exec-only marker before any runtime thread can read the environment.
    unsafe {
        opensided::capture_restart_marker();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("runtime: {error}"));
    if let Err(error) = runtime.block_on(opensided::main()) {
        eprintln!("e2e-opensided: {error}");
        std::process::exit(1);
    }
}
