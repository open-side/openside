//! Emit the exact registry table used by the storage-documentation drift test.
fn main() {
    print!("{}", openside_config::data_layout::markdown());
}
