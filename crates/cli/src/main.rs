use rune_kit_cli::main as library_main;

#[tokio::main]
async fn main() {
    if let Err(e) = library_main().await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
