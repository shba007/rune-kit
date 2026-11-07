use rune_kit::main as library_main;
fn main() {
    if let Some(result) = rune_kit::native_isolation::worker_entry() {
        if let Err(error) = result {
            eprintln!("Native isolation worker failed; plugin was not launched");
            if let Some(code) = error.raw_os_error() {
                eprintln!("Native isolation worker OS error: {code}");
            }
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
