use rune_kit_cli::main as library_main;
fn main() {
    if let Some(result) = rune_kit_core::native_isolation::worker_entry() {
        if result.is_err() {
            eprintln!("Native isolation worker failed; plugin was not launched");
            std::process::exit(1);
        }
        return;
    }
    let runtime = tokio::runtime::Runtime::new().expect("Cannot initialize runtime");
    if let Err(e) = runtime.block_on(library_main()) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
