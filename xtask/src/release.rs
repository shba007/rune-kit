//! Thin integration: Cocogitto owns semantic classification and Markdown rendering.
use crate::cocogitto::{Commit, Generated, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::Path, process::Command};
use toml_edit::DocumentMut;
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const GENERATOR: &str = crate::cocogitto::GENERATOR;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Prepared {
    #[serde(default)]
    pub formatted_published_notes: bool,
    pub base_tag: String,
    pub version: String,
    pub commits: Vec<String>,
    pub notes_sha256: String,
    #[serde(default)]
    pub source_head: Option<String>,
    #[serde(default)]
    pub generator: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Plan {
    pub needed: bool,
    pub reason: String,
    pub version: String,
    pub notes: String,
    pub changelog_section: String,
    pub prepared: Prepared,
}
#[derive(Debug, Serialize)]
pub struct Publication {
    pub ready: bool,
    pub version: String,
    pub reason: String,
}
fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn semver(tag: &str) -> Result<semver::Version> {
    Ok(semver::Version::parse(
        tag.strip_prefix('v').unwrap_or(tag),
    )?)
}
fn config(root: &Path) -> Result<std::path::PathBuf> {
    Ok(root.join("cog.toml"))
}
fn generated(
    _config: std::path::PathBuf,
    base: &str,
    commits: Vec<Commit>,
    timestamp: i64,
) -> Result<Generated> {
    crate::cocogitto::generate(base, commits, timestamp, None)
}
fn render(generated: &Generated) -> Result<(String, String)> {
    let version = generated.releases[0]
        .version
        .as_deref()
        .ok_or("No generated version")?;
    Ok((version.to_string(), generated.markdown.clone()))
}
fn history(root: &Path, base: &str, end: &str) -> Result<Vec<Commit>> {
    let text = output(
        root,
        "git",
        &[
            "log",
            "--no-merges",
            "--format=%H%x00%B%x00%an%x00%ae%x00%ct%x00",
            &format!("{base}..{end}"),
        ],
    )?;
    text.split('\0')
        .collect::<Vec<_>>()
        .as_chunks::<5>()
        .0
        .iter()
        .map(|v| {
            let mut commit = Commit::new(v[0].trim().to_owned(), v[1].trim().to_owned());
            commit.author = Signature {
                name: Some(v[2].into()),
                timestamp: v[4].trim().parse()?,
            };
            Ok(commit)
        })
        .collect()
}
fn baseline(root: &Path) -> Result<String> {
    let main = if output(root, "git", &["rev-parse", "--verify", "origin/main"]).is_ok() {
        "origin/main"
    } else {
        "main"
    };
    let tags = output(root, "git", &["tag", "--merged", main, "--list", "v[0-9]*"])?;
    tags.lines()
        .filter_map(|t| semver(t).ok().map(|v| (v, t)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, t)| t.to_owned())
        .ok_or_else(|| "No release tag reachable from main".into())
}
fn read_prepared(root: &Path) -> Result<Option<Prepared>> {
    match fs::read_to_string(root.join(".release-preparation.json")) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn validate_prepared(root: &Path, prepared: &Prepared, current: &str, end: &str) -> Result<()> {
    if prepared
        .generator
        .as_deref()
        .is_some_and(|g| g != GENERATOR)
    {
        return Err("Unsupported prepared changelog generator".into());
    }
    if prepared.version != current || semver(current)? <= semver(&prepared.base_tag)? {
        return Err("Prepared version is mismatched or does not advance its base".into());
    }
    if let Some(head) = &prepared.source_head {
        output(root, "git", &["merge-base", "--is-ancestor", head, end])?;
    }
    for id in &prepared.commits {
        output(root, "git", &["merge-base", "--is-ancestor", id, end])?;
    }
    let notes = extract(&fs::read_to_string(root.join("CHANGELOG.md"))?, current)?;
    if digest(&notes) != prepared.notes_sha256 {
        return Err("Prepared changelog changed; re-prepare from reviewed history".into());
    }
    let source = history(root, &prepared.base_tag, end)?;
    let timestamp = source.first().map_or(0, |c| c.author.timestamp);
    let cliff = generated(config(root)?, &prepared.base_tag, source, timestamp)?;
    let mut ids: Vec<_> = cliff
        .releases
        .iter()
        .flat_map(|r| r.commits.iter().map(|c| c.id.clone()))
        .collect();
    ids.sort();
    if ids != prepared.commits {
        return Err(
            "Prepared source history is stale; merge an updated release preparation".into(),
        );
    }
    let calculated = cliff
        .releases
        .first()
        .and_then(|r| r.version.as_deref())
        .ok_or("Prepared release has no releasable source")?;
    if semver(calculated)?.to_string() != current {
        return Err("Prepared version does not match Cocogitto calculation".into());
    }
    Ok(())
}
pub fn plan(root: &Path) -> Result<Plan> {
    let current = version(root)?;
    let base = baseline(root)?;
    let source = history(root, &base, "HEAD")?;
    let timestamp = source.first().map_or(0, |c| c.author.timestamp);
    let cliff = generated(config(root)?, &base, source, timestamp)?;
    let mut ids: Vec<_> = cliff
        .releases
        .iter()
        .flat_map(|r| r.commits.iter().map(|c| c.id.clone()))
        .collect();
    ids.sort();
    let mut prepared = Prepared {
        formatted_published_notes: false,
        base_tag: base,
        version: current.clone(),
        commits: ids,
        notes_sha256: String::new(),
        source_head: Some(output(root, "git", &["rev-parse", "HEAD"])?),
        generator: Some(GENERATOR.into()),
    };
    if prepared.commits.is_empty() {
        return Ok(Plan {
            needed: false,
            reason: "No releasable changes".into(),
            version: current,
            notes: String::new(),
            changelog_section: String::new(),
            prepared,
        });
    }
    if let Some(previous) = read_prepared(root)?
        && previous.base_tag == prepared.base_tag
        && previous.commits == prepared.commits
    {
        validate_prepared(root, &previous, &current, "HEAD")?;
        return Ok(Plan {
            needed: false,
            reason: "Reviewed changes already prepared".into(),
            notes: extract(&fs::read_to_string(root.join("CHANGELOG.md"))?, &current)?,
            version: current,
            changelog_section: String::new(),
            prepared: previous,
        });
    }
    let (next, section) = render(&cliff)?;
    if semver(&current)? > semver(&next)? || semver(&next)? <= semver(&prepared.base_tag)? {
        return Err("Calculated release version decreases or does not advance its base".into());
    }
    if output(
        root,
        "git",
        &["rev-parse", "--verify", &format!("refs/tags/v{next}")],
    )
    .is_ok()
    {
        return Err(format!("Release v{next} is already tagged; cannot prepare it again").into());
    }
    let notes = extract(&section, &next)?;
    prepared.version = next.clone();
    prepared.notes_sha256 = digest(&notes);
    Ok(Plan {
        needed: true,
        reason: format!("{GENERATOR} calculated reviewed changes"),
        version: next,
        notes,
        changelog_section: section,
        prepared,
    })
}
pub fn publication(root: &Path) -> Result<Publication> {
    let current = version(root)?;
    let tag = format!("v{current}");
    let tagged = output(
        root,
        "git",
        &["rev-parse", "--verify", &format!("refs/tags/{tag}")],
    )
    .is_ok();
    if tagged {
        output(root, "git", &["merge-base", "--is-ancestor", &tag, "HEAD"])?;
    }
    let Some(prepared) = read_prepared(root)? else {
        if tagged {
            return Ok(Publication {
                ready: false,
                version: current,
                reason: "No pending prepared release (historical tag)".into(),
            });
        }
        return Err("Version is unpublished but prepared metadata is missing; merge release preparation first".into());
    };
    validate_prepared(
        root,
        &prepared,
        &current,
        if tagged { &tag } else { "HEAD" },
    )?;
    if tagged {
        let tagged_notes = extract(
            &output(root, "git", &["show", &format!("{tag}:CHANGELOG.md")])?,
            &current,
        )?;
        let original: Prepared = serde_json::from_str(&output(
            root,
            "git",
            &["show", &format!("{tag}:.release-preparation.json")],
        )?)?;
        if original.version != current || digest(&tagged_notes) != original.notes_sha256 {
            return Err("Existing tag contains different release notes".into());
        }
    }
    Ok(Publication {
        ready: true,
        version: current,
        reason: if tagged {
            "Validated tagged release; publication checks whether a release already exists"
        } else {
            "Valid unpublished prepared release"
        }
        .into(),
    })
}
fn publication_section(current: &str, tagged: Option<&str>, version: &str) -> Result<String> {
    extract(tagged.unwrap_or(current), version)
}
fn formatted_publication_section(
    current: &str,
    prepared: &Prepared,
    version: &str,
) -> Result<String> {
    if !prepared.formatted_published_notes
        || prepared.generator.as_deref() != Some(GENERATOR)
        || prepared.version != version
    {
        return Err(
            "Published-note formatting requires an explicit matching Cocogitto receipt".into(),
        );
    }
    let notes = extract(current, version)?;
    if digest(&notes) != prepared.notes_sha256 {
        return Err("Formatted published notes differ from their reviewed receipt".into());
    }
    Ok(notes)
}
pub fn publication_notes(root: &Path, version: &str) -> Result<String> {
    if let Some(prepared) = read_prepared(root)?
        && prepared.formatted_published_notes
    {
        let tag = format!("v{version}");
        output(
            root,
            "git",
            &["rev-parse", "--verify", &format!("refs/tags/{tag}")],
        )?;
        validate_prepared(root, &prepared, version, &tag)?;
        return formatted_publication_section(
            &fs::read_to_string(root.join("CHANGELOG.md"))?,
            &prepared,
            version,
        );
    }
    if output(
        root,
        "git",
        &["rev-parse", "--verify", &format!("refs/tags/v{version}")],
    )
    .is_ok()
    {
        let tagged = output(root, "git", &["show", &format!("v{version}:CHANGELOG.md")])?;
        return publication_section("", Some(&tagged), version);
    }
    publication_section(
        &fs::read_to_string(root.join("CHANGELOG.md"))?,
        None,
        version,
    )
}
pub fn verify_artifact(root: &Path, event: &serde_json::Value) -> Result<()> {
    let current = version(root)?;
    if event["release"]["tag_name"].as_str() != Some(format!("v{current}").as_str()) {
        return Err("Artifact event tag does not match workspace version".into());
    }
    let notes = publication_notes(root, &current)?;
    let body = event["release"]["body"]
        .as_str()
        .ok_or("Release event body is missing")?
        .replace("\r\n", "\n");
    if body.trim() != notes.trim() {
        return Err("Release event body differs from the exact prepared changelog section".into());
    }
    Ok(())
}
pub fn example() -> Result<String> {
    let mut commit = Commit::new(
        "0123456789abcdef0123456789abcdef01234567".into(),
        "feat(storage): use the hidden user installation root (#123)".into(),
    );
    commit.author = Signature {
        name: Some("Example Contributor".into()),
        timestamp: 1_700_000_000,
    };
    let cliff = generated(
        config(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())?,
        "v0.6.0",
        vec![commit],
        1_700_000_000,
    )?;
    Ok(render(&cliff)?.1)
}
pub fn output(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let result = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

pub fn version(root: &Path) -> Result<String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?.parse::<DocumentMut>()?;
    Ok(manifest["workspace"]["package"]["version"]
        .as_str()
        .ok_or("Missing workspace version")?
        .to_owned())
}

fn bounds(text: &str, version: &str) -> Result<Option<(usize, usize, usize)>> {
    semver::Version::parse(version)?;
    let mut found = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("Changelog generated by")
            && let Some((_, _, end)) = found.as_mut()
            && *end == text.len()
        {
            *end = offset;
        }
        if let Some(title) = line.trim_end().strip_prefix("## ") {
            if let Some((_, _, end)) = found.as_mut()
                && *end == text.len()
            {
                *end = offset;
            }
            let heading = title
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("");
            if heading.trim_start_matches('v') == version {
                if found.is_some() {
                    return Err(format!("Duplicate changelog section {version}").into());
                }
                found = Some((offset, offset + line.len(), text.len()));
            }
        }
        offset += line.len();
    }
    Ok(found)
}

pub fn extract(text: &str, version: &str) -> Result<String> {
    let (_, start, end) =
        bounds(text, version)?.ok_or_else(|| format!("Missing changelog section {version}"))?;
    let normalized = text[start..end].replace("\r\n", "\n");
    let body = normalized
        .split("Changelog generated by")
        .next()
        .unwrap()
        .trim_end()
        .trim_end_matches("- - -")
        .trim();
    let lower = body.to_ascii_lowercase();
    if body.split_whitespace().count() < 3
        || !body.lines().any(|line| line.starts_with("- "))
        || [
            "todo",
            "tbd",
            "placeholder",
            "unreleased",
            "routine release update",
            "nothing yet",
            "coming soon",
        ]
        .iter()
        .any(|word| {
            lower
                .split(|c: char| !c.is_alphanumeric())
                .any(|part| part == *word)
                || lower.contains(word) && word.contains(' ')
        })
    {
        return Err(format!("Empty or placeholder release notes for {version}").into());
    }
    Ok(format!("{body}\n"))
}

pub fn apply(root: &Path, plan: &Plan) -> Result<()> {
    if !plan.needed {
        return Ok(());
    }
    let path = root.join("Cargo.toml");
    let mut manifest = fs::read_to_string(&path)?.parse::<DocumentMut>()?;
    manifest["workspace"]["package"]["version"] = toml_edit::value(&plan.version);
    fs::write(path, manifest.to_string())?;
    let path = root.join("Cargo.lock");
    let mut lock = fs::read_to_string(&path)?.parse::<DocumentMut>()?;
    for package in lock["package"]
        .as_array_of_tables_mut()
        .ok_or("Missing lockfile packages")?
        .iter_mut()
    {
        if matches!(
            package["name"].as_str(),
            Some("rune-kit-cli" | "rune-kit-core")
        ) {
            package["version"] = toml_edit::value(&plan.version);
        }
    }
    fs::write(path, lock.to_string())?;
    let path = root.join("CHANGELOG.md");
    let text = fs::read_to_string(&path)?;
    let updated = if let Some((start, _, end)) = bounds(&text, &plan.version)? {
        format!(
            "{}{}\n- - -\n\n{}",
            &text[..start],
            plan.changelog_section,
            &text[end..]
        )
    } else {
        let at = text.find("\n## ").map(|n| n + 1).unwrap_or(text.len());
        format!(
            "{}{}\n- - -\n\n{}",
            &text[..at],
            plan.changelog_section,
            &text[at..]
        )
    };
    fs::write(path, updated)?;
    fs::write(
        root.join(".release-preparation.json"),
        format!("{}\n", serde_json::to_string_pretty(&plan.prepared)?),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package(base: &str, messages: &[&str]) -> Generated {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        generated(
            config(root).unwrap(),
            base,
            messages
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let mut c = Commit::new(format!("{i:040x}"), (*m).into());
                    c.author = Signature {
                        name: Some("Fixture Author".into()),
                        timestamp: 1_700_000_000,
                    };
                    c
                })
                .collect(),
            1_700_000_000,
        )
        .unwrap()
    }
    #[test]
    fn cocogitto_calculates_explicit_feature_breaking_and_maintenance_policy() {
        for (base, message, expected) in [
            ("v0.6.0", "fix: retain cache", "0.6.1"),
            ("v0.6.0", "perf: bound queue", "0.6.1"),
            ("v0.6.0", "feat: add dashboard", "0.7.0"),
            ("v0.6.0", "fix!: change transport", "0.7.0"),
            ("v1.6.0", "feat: add dashboard", "1.7.0"),
            ("v1.6.0", "docs!: change public contract", "2.0.0"),
            (
                "v1.6.0",
                "fix: change protocol\n\nBREAKING CHANGE: clients must update",
                "2.0.0",
            ),
        ] {
            assert_eq!(
                render(&package(base, &[message])).unwrap().0,
                expected,
                "{message}"
            );
        }
        for kind in [
            "docs", "refactor", "style", "test", "build", "ci", "chore", "revert", "custom",
        ] {
            assert_eq!(
                render(&package(
                    "v0.6.0",
                    &[&format!("{kind}: clarify diagnostics")]
                ))
                .unwrap()
                .0,
                "0.6.1"
            );
        }
        assert_eq!(
            render(&package(
                "v1.6.0",
                &[
                    "feat: add status",
                    "fix!: change API",
                    "docs: explain changes"
                ]
            ))
            .unwrap()
            .0,
            "2.0.0"
        );
    }
    #[test]
    fn package_omits_bookkeeping_and_private_planning_but_renders_substantive_groups() {
        let cliff = package(
            "v0.6.0",
            &[
                "ci(release): update version",
                "docs: finish KIT-13",
                "chore: autoformat source",
                "fix: improve request errors (#12)",
                "docs: explain storage",
                "test: cover migration",
                "build: pin tooling",
                "ci: validate artifacts",
                "refactor: reuse validation",
                "perf: bound memory",
                "chore: improve diagnostics",
            ],
        );
        let (_, text) = render(&cliff).unwrap();
        for group in [
            "🐛 Bug Fixes",
            "📚 Documentation",
            "🧪 Testing",
            "📦 Build",
            "🤖 Continuous Integration",
            "🚜 Refactoring",
            "⚡ Performance",
            "⚙️ Miscellaneous",
        ] {
            assert!(text.contains(&format!("#### {group}")), "{text}");
        }
        assert!(!text.contains("KIT-13"));
        assert!(!text.contains("update version"));
        assert!(!text.contains("### Enhancements"));
        assert!(text.contains("/compare/v0.6.0..v0.6.1"));
        assert!(text.contains("(#12)"));
        assert!(text.contains("/commit/"));
        assert!(
            package("v0.6.0", &["ci(release): update version", "not semantic"])
                .releases
                .iter()
                .all(|r| r.commits.is_empty())
        );
    }
    #[test]
    fn breaking_notes_contributors_and_repeat_generation_are_package_rendered() {
        let cliff = package(
            "v0.6.0",
            &["feat!: change root\n\nBREAKING CHANGE: clients use the hidden installation root"],
        );
        let (version, first) = render(&cliff).unwrap();
        let (_, second) = render(&package(
            "v0.6.0",
            &["feat!: change root\n\nBREAKING CHANGE: clients use the hidden installation root"],
        ))
        .unwrap();
        assert_eq!(version, "0.7.0");
        assert_eq!(first, second);
        assert!(first.contains("**BREAKING CHANGE**:"));
        assert!(first.contains("Fixture Author"));
        let more = package("v0.6.0", &["feat!: change root", "fix: retain originals"]);
        assert_eq!(render(&more).unwrap().0, "0.7.0");
    }
    #[test]
    fn repeated_apply_preserves_published_history_and_exact_note_extraction() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nversion='0.6.0'\n",
        )
        .unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "version=4\n[[package]]\nname='rune-kit-cli'\nversion='0.6.0'\n",
        )
        .unwrap();
        let historical = "## 0.6.0 - earlier\r\n- Keep published notes exactly.\r\n";
        fs::write(
            root.join("CHANGELOG.md"),
            format!("# Changelog\r\n\r\n{historical}"),
        )
        .unwrap();
        let (version, section) = render(&package("v0.6.0", &["feat: display paths"])).unwrap();
        let notes = extract(&section, &version).unwrap();
        let plan = Plan {
            needed: true,
            reason: String::new(),
            version: version.clone(),
            notes: notes.clone(),
            changelog_section: section,
            prepared: Prepared {
                formatted_published_notes: false,
                base_tag: "v0.6.0".into(),
                version: version.clone(),
                commits: vec!["source".into()],
                notes_sha256: digest(&notes),
                source_head: None,
                generator: Some(GENERATOR.into()),
            },
        };
        apply(root, &plan).unwrap();
        let first = fs::read(root.join("CHANGELOG.md")).unwrap();
        apply(root, &plan).unwrap();
        assert_eq!(first, fs::read(root.join("CHANGELOG.md")).unwrap());
        assert!(String::from_utf8(first).unwrap().ends_with(historical));
        assert_eq!(
            extract(
                &fs::read_to_string(root.join("CHANGELOG.md")).unwrap(),
                &version
            )
            .unwrap(),
            notes
        );
        assert!(extract("## v0.7.0\n- TBD feature.", "0.7.0").is_err());
        assert!(
            extract(
                "## v0.7.0\n- First public fix.\n## v0.7.0\n- Duplicate public fix.",
                "0.7.0"
            )
            .is_err()
        );
    }
    #[test]
    fn artifact_uses_the_same_extractor_and_rejects_mismatched_body_or_version() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nversion='0.7.0'\n",
        )
        .unwrap();
        let (_, text) = render(&package("v0.6.0", &["feat: show state"])).unwrap();
        fs::write(root.join("CHANGELOG.md"), &text).unwrap();
        let body = extract(&text, "0.7.0").unwrap();
        let mut event = serde_json::json!({"release":{"tag_name":"v0.7.0","body":body}});
        assert!(verify_artifact(root, &event).is_ok());
        event["release"]["body"] = serde_json::json!("Different notes");
        assert!(verify_artifact(root, &event).is_err());
        event["release"]["tag_name"] = serde_json::json!("v0.8.0");
        assert!(verify_artifact(root, &event).is_err());
    }
    #[test]
    fn tagged_notes_survive_a_local_format_migration() {
        let original = "## v0.7.0\n- Original reviewed release description.\n";
        let migrated = "## [v0.7.0](https://example.test) - 2026-10-24\n#### Features\n- Reformatted local description.\n";
        let expected = extract(original, "0.7.0").unwrap();
        assert_eq!(
            publication_section(migrated, Some(original), "0.7.0").unwrap(),
            expected
        );
        assert_ne!(
            publication_section(migrated, None, "0.7.0").unwrap(),
            expected
        );
        assert!(publication_section(migrated, Some(original), "0.8.0").is_err());
    }
    #[test]
    fn published_formatting_requires_the_explicit_exact_version_and_digest_receipt() {
        let current = "## v0.7.0\n- Reviewed Cocogitto formatting.\n";
        let mut prepared = Prepared {
            formatted_published_notes: true,
            base_tag: "v0.6.0".into(),
            version: "0.7.0".into(),
            commits: vec![],
            source_head: None,
            generator: Some(GENERATOR.into()),
            notes_sha256: digest(&extract(current, "0.7.0").unwrap()),
        };
        assert_eq!(
            formatted_publication_section(current, &prepared, "0.7.0").unwrap(),
            extract(current, "0.7.0").unwrap()
        );
        assert!(formatted_publication_section(current, &prepared, "0.8.0").is_err());
        prepared.formatted_published_notes = false;
        assert!(formatted_publication_section(current, &prepared, "0.7.0").is_err());
        prepared.formatted_published_notes = true;
        prepared.notes_sha256 = "changed".into();
        assert!(formatted_publication_section(current, &prepared, "0.7.0").is_err());
    }
    #[test]
    fn cocogitto_section_excludes_older_versions_and_footer() {
        let document = "# Changelog\n\n- - -\n\n## [v0.7.0](https://example.test) - 2026-10-24\n\n#### Features\n- Current public behavior.\n\n- - -\n\n## v0.6.0 - 2026-10-21\n- Historical public behavior.\n\n- - -\n\nChangelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).\n";
        assert_eq!(
            extract(document, "0.7.0").unwrap(),
            "#### Features\n- Current public behavior.\n"
        );
    }
}
