cargo build --release

cargo install --path crates/rune-kit-cli

cargo run --bin rune -- install ../rune-tools/target/wasm32-wasip1/release/rune_filesystem.wasm

## Test Prompt

* **For `rune-fetch`:**
> "Fetch the text contents from `[https://httpbin.org/html](https://httpbin.org/html)` and summarize what it says."

* **Tools triggered:** `fetch`


* **For `rune-filesystem`:**
> "Write 'Rune FS verification' to `test-write.txt`, list the current directory, and then read `test-read.txt`."

* **Tools triggered:** `write_file`, `list_directory`, `read_file`


* **For `rune-git`:**
> "Check the current git status in the repository, show the last 3 commit logs, and check for any unstaged diffs."

* **Tools triggered:** `git_status`, `git_log`, `git_diff_unstaged`

> "Inspect the staged changes in this repository using Git tools. Formulate a semantic commit message adhering strictly to Conventional Commits / Commitlint format (<type>(<optional scope>): <description> in lowercase, imperative mood, under 100 chars), and commit the changes."

* **Tools triggered:** `git_diff_staged`, `git_commit`


* **For `rune-time`:**
> "What is the current time in London, and what would that exact time convert to in New York?"


* **Tools triggered:** `get_current_time`, `convert_time`




* **For `rune-memory`:**
> "Create an entity for 'Alice' of type 'Person' with observation 'Speaks Rust and TypeScript'. Create an entity for 'Project Rune' of type 'Software'. Link Alice to Project Rune with relation 'maintains'. Then read the entire graph."


* **Tools triggered:** `create_entities`, `create_relations`, `read_graph`

* **For `rune-sequential-thinking`:**

> "Use rune-sequential-thinking mcp to design an optimal database schema for an e-commerce order management system. Break down your reasoning into consecutive thought steps, explore an alternative branch for event-sourcing, and synthesize the final decision."

* **Tools triggered:** `sequentialthinking` across iterative reasoning steps.


* **Video Metadata Inspection (Zero Disk Write):**
> "Inspect the video metadata for `[https://www.youtube.com/shorts/-uS1wD3jYec](https://www.youtube.com/shorts/-uS1wD3jYec)`. Show me the available formats, duration, and channel name."


* **Tool triggered:** `inspect_video_metadata`

### Universal Test Prompts

* **Ink Levels & Printer State (Generic IPP):**
> "Check the printer status and show me the ink/toner tank percentages and paper tray state."


* **Tool:** `printer_get_status`


* **AirScan Document Capture (Generic eSCL):**
> "Scan the page currently on the flatbed scanner at 300 DPI in color and save it to `scans/document.pdf`."


* **Tool:** `printer_scan_document`


* **Print Document (Generic IPP):**
> "Print 2 copies of `invoice.pdf` in black and white on A4 paper."


* **Tool:** `printer_print_document`


**Individual Diagnostic Prompts (For Granular Testing)**

* **Device & Marker Status Check:**
> "Call `printer_get_status` on `rune-print` to fetch the CMYK marker levels and printer state."
> 
> 


* **Tool:** `printer_get_status`



* **AirScan Capabilities Probe:**
> "Call `printer_get_scanner_capabilities` to inspect the raw eSCL XML capabilities of the scanner."
> 
> 


* **Tool:** `printer_get_scanner_capabilities`



* **Platen Scan (JPEG):**
> "Call `printer_scan_document` with `outputPath: "scans/test.jpg"`, `resolutionDpi: 300`, `colorMode: "Color"`, `inputSource: "Platen"`."
> 
> 


* **Tool:** `printer_scan_document`



* **Platen Scan (Auto-PDF Conversion):**
> "Call `printer_scan_document` with `outputPath: "scans/document.pdf"`, `resolutionDpi: 150`, `colorMode: "Grayscale"`, `inputSource: "Platen"`."
> 
> 


* **Tool:** `printer_scan_document`



* **Print Queue Inspection:**
> "Call `printer_get_jobs` with `whichJobs: "all"` to view the IPPS job history."
> 
> 


* **Tool:** `printer_get_jobs`



* **Print Document Dispatch:**
> "Call `printer_print_document` to print 1 copy of `scans/test.jpg` on A4 paper in monochrome."
> 
> 


* **Tool:** `printer_print_document`



* **Maintenance Trigger:**
> "Call `printer_run_vendor_maintenance` with `action: "clean_printheads_level1"`."
> 
> 


* **Tool:** `printer_run_vendor_maintenance`

## Test CLI

bunx @modelcontextprotocol/inspector --tui -- D:/Projects/Practice/rune-kit/target/release/rune.exe run rune_fs -p allowed_dir=D:/Projects/Practice/rune-kit/test-dir


