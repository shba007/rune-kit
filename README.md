cargo build --release

cargo install --path crates/rune-kit-cli

cargo run --bin rune -- install ../rune-tools/target/wasm32-wasip1/release/<rune-tool-name>.wasm

## Test Prompt

**Individual Prompts**

* **For `rune-git`:**
> "Check the current git status in the repository, show the last 3 commit logs, and check for any unstaged diffs."


* **Tools triggered:** `git_status`, `git_log`, `git_diff_unstaged`

* **For `rune-time`:**
> "What is the current time in London, and what would that exact time convert to in New York?"


* **Tools triggered:** `get_current_time`, `convert_time`


* **For `rune-fs`:**
> "Write 'Rune FS verification' to `test.txt`, list the current directory, and then read `test-read.txt`."


* **Tools triggered:** `write_file`, `list_directory`, `read_file`

* **For `rune-fetch`:**
> "Fetch the text contents from `[https://httpbin.org/html](https://httpbin.org/html)` and summarize what it says."


* **Tools triggered:** `fetch`

* **For `rune-memory`:**
> "Create an entity for 'Alice' of type 'Person' with observation 'Speaks Rust and TypeScript'. Create an entity for 'Project Rune' of type 'Software'. Link Alice to Project Rune with relation 'maintains'. Then read the entire graph."


* **Tools triggered:** `create_entities`, `create_relations`, `read_graph`


## Test CLI

bunx @modelcontextprotocol/inspector --tui -- D:/Projects/Practice/rune-kit/target/release/rune.exe run rune_fs -p allowed_dir=D:/Projects/Practice/rune-kit/test-dir


