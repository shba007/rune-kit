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
        let configured = std::env::var("RUNE_FIXTURE_TOOLS");
        let result = if line.contains("tools/list") {
            configured.as_deref().unwrap_or(
            r#"{"tools":[{"name":"read","description":"Read fixture","inputSchema":{"type":"object"}},{"name":"write","description":"Write fixture","inputSchema":{"type":"object"}}]}"#)
        } else if line.contains("resources/list") {
            r#"{"resources":[]}"#
        } else {
            std::thread::sleep(std::time::Duration::from_secs(10));
            r#"{"content":[]}"#
        };
        println!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}");
    }
}
