//! Daemon diagnostics on stderr that cannot take the process down.
//!
//! `eprintln!` panics when stderr is a pipe whose reader is gone (EPIPE). A panic inside the
//! panic hook, which also reports on stderr, aborts the process with SIGABRT.

#[doc(hidden)]
pub fn write_stderr(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    // A diagnostic nobody can read is not worth failing for.
    let _ = writeln!(std::io::stderr().lock(), "{args}");
}

/// `eprintln!` that ignores write errors (closed pipe, full disk, revoked tty).
#[macro_export]
macro_rules! diag {
    ($($arg:tt)*) => {
        $crate::diag::write_stderr(format_args!($($arg)*))
    };
}
