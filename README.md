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

---

* **For `rune-image` (Metadata & Preview Inspection):**

> "Inspect the image gallery using rune-image at `[https://www.reddit.com/r/LocalLLaMA/comments/1ve4uoe/daniel_han_of_unsloth_validates_qwen3827b_will/](https://www.reddit.com/r/LocalLLaMA/comments/1ve4uoe/daniel_han_of_unsloth_validates_qwen3827b_will/)` to check the total number of media files and display preview links."

* **Tools triggered:** `inspect_image_gallery`

* **For `rune-image` (Discovery & Filtered Batch Ingestion):**

> "Inspect the album at `[https://www.reddit.com/r/LocalLLaMA/comments/1ve4uoe/daniel_han_of_unsloth_validates_qwen3827b_will/](https://www.reddit.com/r/LocalLLaMA/comments/1ve4uoe/daniel_han_of_unsloth_validates_qwen3827b_will/)`, then download the first 5 images (`filterRange: '1-5'`) into `./test-dir/images` using the cookie files in `./test-dir/cookies`."

* **Tools triggered:** `inspect_image_gallery`, `download_image_collection`

---

**MCP Configuration (`mcp.json`)**

```json
{
  "mcpServers": {
    "rune-audio": {
      "command": "D:\\Projects\\Practice\\rune-kit\\target\\release\\rune.exe",
      "args": [
        "run",
        "rune_audio"
      ],
      "env": {
        "COOKIES_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\cookies",
        "OUTPUT_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\audio",
        "ALLOWED_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir"
      }
    }
  }
}

```

*Direct Native Binary Alternative (bypassing WASM runtime):*

```json
{
  "mcpServers": {
    "rune-audio": {
      "command": "D:\\Projects\\Practice\\rune-tools\\target\\release\\rune-audio-native.exe",
      "args": [],
      "env": {
        "COOKIES_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\cookies",
        "OUTPUT_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\audio",
        "ALLOWED_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir"
      }
    }
  }
}

```

---

* **Test Prompt 2 (Alternative Format & Explicit Cookie Path)**:
> *"Convert the audio from `[https://www.youtube.com/shorts/EqvgsORpbOU](https://www.youtube.com/shorts/EqvgsORpbOU)` into Opus format and explicitly load cookies from `D:\Projects\Practice\rune-kit\test-dir\cookies\youtube.txt`."*


extract_audio_track


* **Test Prompt 3 (Spotify Track with Synced Lyrics via spotdl)**:
> *"Download the track `[https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT](https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT)` with synced LRC lyrics and save it to the audio downloads directory."*


download_music_track

---

**MCP Configuration (`mcp.json`)**

```json
{
  "mcpServers": {
    "rune-video": {
      "command": "D:\\Projects\\Practice\\rune-kit\\target\\release\\rune.exe",
      "args": [
        "run",
        "rune_video"
      ],
      "env": {
        "COOKIES_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\cookies",
        "OUTPUT_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\video",
        "ALLOWED_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir"
      }
    }
  }
}

```

*Direct Native Sidecar Binary Alternative:*

```json
{
  "mcpServers": {
    "rune-video": {
      "command": "D:\\Projects\\Practice\\rune-tools\\target\\release\\rune-video-native.exe",
      "args": [],
      "env": {
        "COOKIES_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\cookies",
        "OUTPUT_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir\\video",
        "ALLOWED_DIR": "D:\\Projects\\Practice\\rune-kit\\test-dir"
      }
    }
  }
}

```

---


* **Test Prompt 1 (Metadata Inspection with Auto-Matched Cookies)**:
> *"Inspect the video details and available format codecs for `[https://www.youtube.com/shorts/EqvgsORpbOU](https://www.youtube.com/shorts/EqvgsORpbOU)` using the configured cookies directory."*


inspect_video_metadata


* **Test Prompt 2 (Resolution-Capped Video Download with Subtitles)**:
> *"Download the video at `[https://www.youtube.com/shorts/EqvgsORpbOU](https://www.youtube.com/shorts/EqvgsORpbOU)` capped at 720p resolution, write English subtitles, and save it to the default video folder."*


download_video_stream

* **Test Prompt 3 (Batch Playlist Download with Range Limit)**:
> *"Download the first 3 items from the playlist `[https://www.youtube.com/playlist?list=PLrAXtmErZgOdP_8GzKt42n_k6_k7d-f6X](https://www.youtube.com/playlist?list=PLrAXtmErZgOdP_8GzKt42n_k6_k7d-f6X)` starting from index 1."*


download_video_playlist

* **Test Prompt 4 (Lossless FFmpeg Clip Trimming)**:
> *"Trim the video file at `D:\Projects\Practice\rune-kit\test-dir\video\sample.mp4` from timestamp `00:00:05` to `00:00:15` without re-encoding, saving it as `D:\Projects\Practice\rune-kit\test-dir\video\clip.mp4`."*


trim_media_clip

```

* **Test Prompt 5 (Live Broadcast Recording via Streamlink)**:
> *"Record 2 minutes of the live stream at `[https://twitch.tv/monstercat](https://twitch.tv/monstercat)` in best quality and save it to the output directory."*


record_live_stream
```
---

**Email Operations & Testing Prompts**

**Connectivity & Discovery**

* *"Verify my email connection and check how many total mailboxes and folders exist on the mail server."*
* **Tool:** `verify_email_connection`

**Listing & Searching**

* *"List the 5 most recent unread emails in my INBOX and show me who sent them and the subject lines."*
* **Tool:** `list_messages`
* **Payload:** `{"mailbox": "INBOX", "limit": 5, "unreadOnly": true}`


* *"Search my INBOX for all emails received from 'GitHub' or with 'security' in the subject line."*
* **Tool:** `search_messages`
* **Payload:** `{"mailbox": "INBOX", "from": "GitHub", "subject": "security", "limit": 10}`


* *"Find any emails in 'INBOX' sent after '2026-08-01' that mention 'invoice' in the body."*
* **Tool:** `search_messages`
* **Payload:** `{"mailbox": "INBOX", "query": "invoice", "sinceDate": "2026-08-01"}`

**Reading & Attachment Handling**

* *"Read email UID 1042 from INBOX, show me its plain text content, and mark it as read."*
* **Tool:** `read_message`
* **Payload:** `{"uid": 1042, "mailbox": "INBOX", "markAsRead": true}`

* *"Download the attachment named 'invoice_august.pdf' from email UID 1042 into my downloads directory."*
* **Tool:** `download_attachment`
* **Payload:** `{"uid": 1042, "mailbox": "INBOX", "filename": "invoice_august.pdf"}`

**Sending, Replying & Drafting**

* *"Send an email to `alex@example.com` with subject 'Project Alpha Status' and body 'The sprint deliverables are ready for review.' Ensure a copy is saved to my Sent folder."*
* **Tool:** `send_email`
* **Payload:** `{"to": "alex@example.com", "subject": "Project Alpha Status", "bodyText": "The sprint deliverables are ready for review."}`


* *"Send a threaded reply to email UID 1042 in INBOX saying 'Thank you, I have received the documents.' Do not reply to all."*
* **Tool:** `reply_email`
* **Payload:** `{"originalUid": 1042, "mailbox": "INBOX", "replyBody": "Thank you, I have received the documents.", "replyAll": false}`


* *"Save a draft email to `client@domain.com` with subject 'Contract Agreement Draft' and body 'Please find the revised agreement terms below.' without sending it."*
* **Tool:** `draft_email`
* **Payload:** `{"to": "client@domain.com", "subject": "Contract Agreement Draft", "bodyText": "Please find the revised agreement terms below."}`



**Mailbox Organization**

* *"Star email UID 1042 in INBOX and mark it as unread."*
* **Tool:** `manage_message_flags`
* **Payload:** `{"uid": 1042, "mailbox": "INBOX", "action": "star"}`


* *"Move email UID 1042 from 'INBOX' to 'Archive'."*
* **Tool:** `move_message`
* **Payload:** `{"uid": 1042, "sourceMailbox": "INBOX", "destinationMailbox": "Archive"}`



---

**End-to-End Agent Workflow Prompts**

* **Invoice Ingestion Workflow:**
> *"Check my INBOX for any unread email with subject containing 'Receipt' or 'Invoice'. Read the email, download all PDF attachments to the output directory, star the email, and send a reply saying 'Received and archived, thanks!'"*


* **Daily Digest Summary:**
> *"Scan the top 10 most recent emails from my INBOX, summarize the key action items from each sender in a Markdown table, and tell me if any require immediate reply."*

---

* **For `rune-time`:**
> "What is the current time in London, and what would that exact time convert to in New York?"

* **Tools triggered:** `get_current_time`, `convert_time`


* **For `rune-memory`:**
> "Create an entity for 'Alice' of type 'Person' with observation 'Speaks Rust and TypeScript'. Create an entity for 'Project Rune' of type 'Software'. Link Alice to Project Rune with relation 'maintains'. Then read the entire graph."


* **Tools triggered:** `create_entities`, `create_relations`, `read_graph`

* **For `rune-sequential-thinking`:**

> "Use rune-sequential-thinking mcp to design an optimal database schema for an e-commerce order management system. Break down your reasoning into consecutive thought steps, explore an alternative branch for event-sourcing, and synthesize the final decision."

* **Tools triggered:** `sequentialthinking` across iterative reasoning steps.


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


