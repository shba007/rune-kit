use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::Path, process::Command};
use toml_edit::DocumentMut;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug, Deserialize)]
struct Commit {
    id: String,
    summary: String,
    body: Option<String>,
    scope: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    breaking_change: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Prepared {
    pub base_tag: String,
    pub version: String,
    pub commits: Vec<String>,
    pub notes_sha256: String,
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
                .trim_matches(['[', ']']);
            if heading == version {
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
    let body = text[start..end].trim();
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

fn public_text(text: &str) -> bool {
    !text
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .any(|word| {
            let lower = word.to_ascii_lowercase();
            ["cycle", "milestone"].iter().any(|prefix| {
                lower.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.is_empty()
                        || suffix == "s"
                        || suffix
                            .trim_start_matches('-')
                            .chars()
                            .all(|c| c.is_ascii_digit())
                })
            }) || lower.starts_with("kit-") && lower[4..].chars().all(|c| c.is_ascii_digit())
                || lower.starts_with("kf-") && lower[3..].chars().all(|c| c.is_ascii_digit())
                || (lower.starts_with('k') || lower.starts_with('m'))
                    && lower.len() > 1
                    && lower[1..].chars().all(|c| c.is_ascii_digit())
        })
}

fn releasable(commit: &Commit) -> bool {
    let summary = commit.summary.to_ascii_lowercase();
    public_text(&commit.summary)
        && commit.scope.as_deref() != Some("release")
        && commit.scope.as_deref() != Some("version")
        && !summary.starts_with("update version")
        && !summary.starts_with("bump version")
        && (commit.breaking_change
            || matches!(
                commit.kind.as_str(),
                "🚀 Features" | "🐛 Bug Fixes" | "⚡ Performance"
            ))
}

fn notes(commits: &[Commit]) -> String {
    let mut lines = Vec::new();
    for commit in commits.iter().filter(|c| releasable(c)) {
        let mut entries = vec![commit.summary.as_str()];
        if let Some(body) = &commit.body {
            entries.extend(body.lines().filter_map(|line| {
                line.strip_prefix("Release-note: ")
                    .or_else(|| line.strip_prefix("Migration: "))
            }));
        }
        for text in entries {
            let lower = text.to_ascii_lowercase();
            if !public_text(text)
                || ["added tests", "documented", "created shared wire fixtures"]
                    .iter()
                    .any(|prefix| lower.starts_with(prefix))
            {
                continue;
            }
            let entry = format!("- {}", text.trim());
            if !lines.contains(&entry) {
                lines.push(entry);
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn base_version(tag: &str) -> Result<semver::Version> {
    Ok(semver::Version::parse(tag.trim_start_matches('v'))?)
}
fn next_version(mut version: semver::Version, commits: &[Commit]) -> semver::Version {
    let breaking = commits
        .iter()
        .filter(|c| releasable(c))
        .any(|c| c.breaking_change);
    let feature = commits
        .iter()
        .filter(|c| releasable(c))
        .any(|c| c.kind == "🚀 Features");
    if breaking && version.major > 0 {
        version.major += 1;
        version.minor = 0;
        version.patch = 0;
    } else if breaking || feature {
        version.minor += 1;
        version.patch = 0;
    } else {
        version.patch += 1;
    }
    version.pre = semver::Prerelease::EMPTY;
    version.build = semver::BuildMetadata::EMPTY;
    version
}

fn previous_notes(
    previous: &Prepared,
    candidate: &Prepared,
    current: &str,
    changelog: &str,
) -> Result<Option<String>> {
    if previous.base_tag != candidate.base_tag || previous.commits != candidate.commits {
        return Ok(None);
    }
    if previous.version != current {
        return Err("Prepared version does not match workspace".into());
    }
    let notes = extract(changelog, current)?;
    if digest(&notes) != previous.notes_sha256 {
        return Err(
            "Prepared changelog has changed; prepare it again from reviewed history".into(),
        );
    }
    Ok(Some(notes))
}

pub fn plan(root: &Path) -> Result<Plan> {
    let current = version(root)?;
    // A main merge tag need not be an ancestor of develop. Use main's highest
    // release tag as the exclusion range, rather than develop's nearest tag.
    let main = if output(root, "git", &["rev-parse", "--verify", "origin/main"]).is_ok() {
        "origin/main"
    } else {
        "main"
    };
    let tags = output(root, "git", &["tag", "--merged", main, "--list", "v[0-9]*"])?;
    let base = tags
        .lines()
        .filter_map(|tag| {
            semver::Version::parse(tag.trim_start_matches('v'))
                .ok()
                .map(|version| (version, tag))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, tag)| tag.to_owned())
        .ok_or("No release tag reachable from main")?;
    let data = output(
        root,
        "cog",
        &[
            "changelog",
            &format!("{base}..HEAD"),
            "--template",
            ".github/release-commits.tera",
        ],
    )?;
    let commits: Vec<Commit> = serde_json::from_str(&data)?;
    let mut ids: Vec<_> = commits
        .iter()
        .filter(|c| releasable(c))
        .map(|c| c.id.clone())
        .collect();
    ids.sort();
    let old = root.join(".release-preparation.json");
    let previous: Option<Prepared> = if old.exists() {
        Some(serde_json::from_str(&fs::read_to_string(old)?)?)
    } else {
        None
    };
    let mut prepared = Prepared {
        base_tag: base,
        version: current.clone(),
        commits: ids,
        notes_sha256: String::new(),
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
    if let Some(previous) = previous
        && let Some(notes) = previous_notes(
            &previous,
            &prepared,
            &current,
            &fs::read_to_string(root.join("CHANGELOG.md"))?,
        )?
    {
        return Ok(Plan {
            needed: false,
            reason: "Reviewed changes already prepared".into(),
            version: current,
            notes,
            changelog_section: String::new(),
            prepared: previous,
        });
    }
    let next = next_version(base_version(&prepared.base_tag)?, &commits).to_string();
    if semver::Version::parse(&current)? > semver::Version::parse(&next)? {
        return Err("Workspace version is ahead of calculated version".into());
    }
    let mut notes = notes(&commits);
    // Preserve reviewed migration guidance from an existing, unpublished section.
    // Read it from HEAD, never from uncommitted working-tree prose.
    if let Ok(reviewed) = output(root, "git", &["show", "HEAD:CHANGELOG.md"])
        && let Ok(existing) = extract(&reviewed, &next)
    {
        for line in existing.lines().filter(|line| line.starts_with("- ")) {
            let lower = line.to_ascii_lowercase();
            if public_text(line)
                && !lower.contains("source commits")
                && !lower.contains("release branches")
                && !notes.lines().any(|entry| entry == line)
            {
                notes.push_str(line);
                notes.push('\n');
            }
        }
    }
    let date = output(
        root,
        "git",
        &[
            "show",
            "-s",
            "--format=%cs",
            &commits
                .iter()
                .find(|c| releasable(c))
                .ok_or("Missing source commit")?
                .id,
        ],
    )?;
    let section = format!("## {next} - {date}\n{notes}\n");
    extract(&section, &next)?;
    prepared.version = next.clone();
    prepared.notes_sha256 = digest(&notes);
    Ok(Plan {
        needed: true,
        reason: "Reviewed changes require preparation".into(),
        version: next,
        notes,
        changelog_section: section,
        prepared,
    })
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
            "{}{}{}",
            &text[..start],
            plan.changelog_section,
            &text[end..]
        )
    } else {
        let at = text.find("\n## ").map(|n| n + 1).unwrap_or(text.len());
        format!("{}{}{}", &text[..at], plan.changelog_section, &text[at..])
    };
    fs::write(path, updated)?;
    fs::write(
        root.join(".release-preparation.json"),
        format!("{}\n", serde_json::to_string_pretty(&plan.prepared)?),
    )?;
    Ok(())
}

pub fn check(root: &Path) -> Result<bool> {
    let current = version(root)?;
    let tag = format!("v{current}");
    let exists = Command::new("git")
        .args(["rev-parse", "--verify", &format!("refs/tags/{tag}")])
        .current_dir(root)
        .output()?
        .status
        .success();
    if exists {
        output(root, "git", &["merge-base", "--is-ancestor", &tag, "HEAD"])?;
    }
    let plan = plan(root)?;
    if exists && plan.prepared.commits.is_empty() {
        let prepared: Prepared =
            serde_json::from_str(&fs::read_to_string(root.join(".release-preparation.json"))?)?;
        let notes = extract(&fs::read_to_string(root.join("CHANGELOG.md"))?, &current)?;
        if prepared.version != current || prepared.notes_sha256 != digest(&notes) {
            return Err("Existing release tag does not match prepared version/notes".into());
        }
        output(root, "git", &["merge-base", "--is-ancestor", &tag, "HEAD"])?;
        return Ok(true); // Publication checks whether the remote release already exists.
    }
    if plan.needed {
        return Err(
            "Release has not been prepared and reviewed; merge its preparation PR first".into(),
        );
    }
    if plan.prepared.commits.is_empty() {
        return Err("No prepared release metadata".into());
    }
    extract(&fs::read_to_string(root.join("CHANGELOG.md"))?, &current)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_exact_version_and_rejects_bad_sections() {
        let text = "# Changelog\n\n## 0.5.0 - today\n- Useful public functionality.\n\n## 0.4.0\n- Older public functionality.\n";
        assert_eq!(
            extract(text, "0.5.0").unwrap(),
            "- Useful public functionality.\n"
        );
        for text in [
            "## 0.4.0\n- Older public functionality.",
            "## 0.5.0\n",
            "## 0.5.0\n- TBD feature.",
            "## 0.5.0\n- First public fix.\n## 0.5.0\n- Second public fix.",
        ] {
            assert!(extract(text, "0.5.0").is_err());
        }
    }
    #[test]
    fn excludes_bookkeeping_and_internal_labels() {
        for summary in [
            "update version",
            "finish Cycle 2",
            "finish Cycle2",
            "finish milestone-1",
            "prepare M1",
            "ship KIT-13",
            "finish milestone",
            "prepare K1",
        ] {
            let commit = Commit {
                id: "a".into(),
                summary: summary.into(),
                body: None,
                scope: None,
                kind: "🚀 Features".into(),
                breaking_change: false,
            };
            assert!(!releasable(&commit));
        }
    }
    #[test]
    fn versions_advance_from_main_release_even_when_develop_has_an_older_ancestor_tag() {
        let mut commit = Commit {
            id: "feature".into(),
            summary: "Add inventory".into(),
            body: None,
            scope: None,
            kind: "🚀 Features".into(),
            breaking_change: false,
        };
        assert_eq!(
            next_version(base_version("v0.5.0").unwrap(), &[commit]).to_string(),
            "0.6.0"
        );
        commit = Commit {
            id: "fix".into(),
            summary: "Fix inventory".into(),
            body: None,
            scope: None,
            kind: "🐛 Bug Fixes".into(),
            breaking_change: false,
        };
        assert_eq!(
            next_version(base_version("v0.5.0").unwrap(), &[commit]).to_string(),
            "0.5.1"
        );
        commit = Commit {
            id: "break".into(),
            summary: "Change contract".into(),
            body: None,
            scope: None,
            kind: "🚀 Features".into(),
            breaking_change: true,
        };
        assert_eq!(
            next_version(base_version("v1.2.3").unwrap(), &[commit]).to_string(),
            "2.0.0"
        );
    }
    #[test]
    fn public_notes_use_explicit_guidance_instead_of_developer_body_bullets() {
        let commit=Commit { id:"source".into(),summary:"Add terminal inventory".into(),body:Some("- Refactor internal bookkeeping\nRelease-note: Inspect installed plugin versions.\nMigration: Existing storage remains intact.\nRelease-note: Complete Cycle 3".into()),scope:None,kind:"🚀 Features".into(),breaking_change:false };
        let result = notes(&[commit]);
        assert!(result.contains("Add terminal inventory"));
        assert!(result.contains("Inspect installed plugin versions"));
        assert!(result.contains("Existing storage remains intact"));
        assert!(!result.contains("bookkeeping"));
        assert!(!result.contains("Cycle"));
    }
    #[test]
    fn reviewed_source_set_prevents_preparation_loops_and_detects_tampering() {
        let notes = "- Useful public functionality.\n";
        let previous = Prepared {
            base_tag: "v0.4.0".into(),
            version: "0.5.0".into(),
            commits: vec!["source".into()],
            notes_sha256: digest(notes),
        };
        let text = format!("## 0.5.0 - today\n{notes}");
        assert_eq!(
            previous_notes(&previous, &previous, "0.5.0", &text)
                .unwrap()
                .unwrap(),
            notes
        );
        assert!(
            previous_notes(
                &previous,
                &previous,
                "0.5.0",
                "## 0.5.0\n- Changed after preparation.\n"
            )
            .is_err()
        );
        assert!(previous_notes(&previous, &previous, "0.6.0", &text).is_err());
        let mut changed = previous.clone();
        changed.commits.push("new-feature".into());
        assert!(
            previous_notes(&previous, &changed, "0.5.0", &text)
                .unwrap()
                .is_none()
        );
        changed.commits.clear();
        assert!(
            previous_notes(&previous, &changed, "0.5.0", &text)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn applying_same_preparation_twice_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace.package]\nversion = '0.4.0'\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("Cargo.lock"),
            "version = 4\n[[package]]\nname = 'rune-kit-cli'\nversion = '0.4.0'\n",
        )
        .unwrap();
        let history = "## 0.4.0 - yesterday\n- Preserve this published entry.\n";
        fs::write(
            dir.path().join("CHANGELOG.md"),
            format!("# Changelog\n\n{history}"),
        )
        .unwrap();
        let plan = Plan {
            needed: true,
            reason: String::new(),
            version: "0.5.0".into(),
            notes: "- Useful terminal inventory.\n".into(),
            changelog_section: "## 0.5.0 - today\n- Useful terminal inventory.\n\n".into(),
            prepared: Prepared {
                base_tag: "v0.4.0".into(),
                version: "0.5.0".into(),
                commits: vec!["abc".into()],
                notes_sha256: String::new(),
            },
        };
        apply(dir.path(), &plan).unwrap();
        let first = fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
        apply(dir.path(), &plan).unwrap();
        assert_eq!(
            first,
            fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap()
        );
        assert!(first.ends_with(history));
        assert_eq!(extract(&first, "0.5.0").unwrap(), plan.notes);
    }
}
