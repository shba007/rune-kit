use rune_kit_cli::main as library_main;

#[tokio::main]
async fn main() {
    let _ = library_main().await;
}
