cargo build --release

cargo run --bin rune -- install ../rune-tools/target/wasm32-wasip1/release/<rune-tool-name>.wasm

cargo install --path crates/rune-kit-cli