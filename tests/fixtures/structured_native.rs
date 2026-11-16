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
        let tools = std::env::var("RUNE_FIXTURE_TOOLS").unwrap_or_else(|_| r#"{"tools":[{"name":"count","description":"Count fixture","inputSchema":{"type":"object"},"outputSchema":{"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]}}]}"#.into());
        let call = std::env::var("RUNE_FIXTURE_RESULT").unwrap_or_else(|_| r#"{"content":[{"type":"text","text":"Found 3 items"}],"structuredContent":{"count":3},"isError":false}"#.into());
        let call = if call == "oversized" {
            format!(
                r#"{{"content":[],"structuredContent":{{"payload":"{}"}}}}"#,
                "x".repeat(8 * 1024 * 1024)
            )
        } else if call == "large-legacy" {
            format!(
                r#"{{"content":[{{"type":"text","text":"{}"}}]}}"#,
                "x".repeat(8 * 1024 * 1024 + 1)
            )
        } else {
            call
        };
        let result = if line.contains("tools/list") {
            tools.as_str()
        } else if line.contains("tools/call") {
            call.as_str()
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
