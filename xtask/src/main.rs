mod release;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("conformance") => {
            if args.len() != 5 {
                return Err(
                    "Usage: cargo xtask conformance BINARY GIT_NATIVE TIME_WASM FILESYSTEM_WASM"
                        .into(),
                );
            }
            let result = xtask::wire::conformance(
                std::path::Path::new(&args[1]),
                std::path::Path::new(&args[2]),
                std::path::Path::new(&args[3]),
                std::path::Path::new(&args[4]),
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("release-example") => println!("{}", release::example()?),
        Some("release-plan") => {
            let plan = release::plan(root)?;
            if args.iter().any(|a| a == "--apply") {
                release::apply(root, &plan)?;
            }
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        Some("release-notes") => {
            let version = args
                .get(1)
                .ok_or("Usage: cargo xtask release-notes VERSION [--output PATH]")?;
            let notes = release::extract(
                &std::fs::read_to_string(root.join("CHANGELOG.md"))?,
                version,
            )?;
            if let Some(at) = args.iter().position(|a| a == "--output") {
                std::fs::write(args.get(at + 1).ok_or("Missing output path")?, notes)?;
            } else {
                print!("{notes}");
            }
        }
        Some("release-check") => {
            println!("{}", serde_json::to_string(&release::publication(root)?)?)
        }
        Some("release-artifact-check") => {
            let event: serde_json::Value = serde_json::from_slice(&std::fs::read(
                std::env::var_os("GITHUB_EVENT_PATH").ok_or("GITHUB_EVENT_PATH is required")?,
            )?)?;
            release::verify_artifact(root, &event)?;
            println!("Artifact tag and release body match prepared notes");
        }
        _ => {
            return Err(
                "Use cargo xtask release-plan [--apply], release-notes VERSION, or release-check"
                    .into(),
            );
        }
    }
    Ok(())
}
