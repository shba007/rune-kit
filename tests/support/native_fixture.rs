use std::io::{self, BufRead};
fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let tail = line.split("\"id\":").nth(1).unwrap();
        let id: String = tail
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if std::env::var_os("KIT_FIXTURE_HANG").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(20));
        }
        let configured = std::env::var("RUNE_FIXTURE_TOOLS");
        let result = if line.contains("tools/list") {
            configured.as_deref().unwrap_or(r#"{"tools":[{"name":"read","description":"Read-only fixture","inputSchema":{"type":"object","x-rune-rules":{"effects":["read"]}},"annotations":{"readOnlyHint":true,"readOnly":true}},{"name":"slow","description":"Long call","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},{"name":"write","description":"Write fixture","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":false}}]}"#)
        } else if line.contains("tools/call") {
            if line.contains("\"slow\"") {
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
            r#"{"content":[{"type":"text","text":"fixture response"}],"isError":false}"#
        } else if line.contains("resources/list") {
            r#"{"resources":[]}"#
        } else if line.contains("prompts/list") {
            r#"{"prompts":[]}"#
        } else {
            r#"{"name":"fixture","version":"0.1.0"}"#
        };
        println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}");
    }
}
