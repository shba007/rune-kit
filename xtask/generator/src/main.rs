mod engine;
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let request: serde_json::Value = serde_json::from_reader(std::io::stdin())?;
    let commits: Vec<engine::Commit> = serde_json::from_value(request["commits"].clone())?;
    let timestamp = request["timestamp"].as_i64().ok_or("Missing timestamp")?;
    let result = if request["operation"] == "render" {
        serde_json::to_value(engine::render_at(
            request["base"].as_str(),
            request["version"].as_str().ok_or("Missing version")?,
            &commits,
            timestamp,
            None,
        )?)?
    } else {
        serde_json::to_value(engine::generate(
            request["base"].as_str().ok_or("Missing base")?,
            commits,
            timestamp,
            None,
        )?)?
    };
    serde_json::to_writer(std::io::stdout(), &result)?;
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
