cargo build --release

cargo run --bin rune -- install ../rune-tools/target/wasm32-wasip1/release/<rune-tool-name>.wasm

cargo install --path crates/rune-kit-cli

## Test Prompt

**Individual Prompts**

* **For `rune-time`:**
> "What is the current time in London, and what would that exact time convert to in New York?"


* **Tools triggered:** `get_current_time`, `convert_time`


* **For `rune-fs`:**
> "Write 'Rune FS verification' to `test.txt`, list the current directory, and then read `test-read.txt`."


* **Tools triggered:** `write_file`, `list_directory`, `read_file`

* **For `rune-fetch`:**
> "Fetch the text contents from `[https://httpbin.org/html](https://httpbin.org/html)` and summarize what it says."


* **Tools triggered:** `fetch`


## Test CLI

bunx @modelcontextprotocol/inspector --tui -- D:/Projects/Practice/rune-kit/target/release/rune.exe run rune_fs -p allowed_dir=D:/Projects/Practice/rune-kit/test-dir


