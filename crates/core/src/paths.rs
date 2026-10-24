//! Shared Kit storage. The public gateway and private plugin ABI use the same paths.
use std::io;
use std::path::{Path, PathBuf};

pub fn managed_root() -> io::Result<PathBuf> {
    if let Some(value) = std::env::var_os("RUNE_KIT_DATA_DIR") {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(io::Error::other("RUNE_KIT_DATA_DIR must be absolute"));
        }
        return Ok(path);
    }
    let home = dirs::home_dir()
        .filter(|p| p.is_absolute())
        .ok_or_else(|| io::Error::other("Cannot resolve user home"))?;
    default_root(&home, dirs::data_dir().as_deref())
}

pub fn installation_root() -> io::Result<PathBuf> {
    dirs::home_dir()
        .filter(|p| p.is_absolute())
        .map(|p| p.join(".rune"))
        .ok_or_else(|| io::Error::other("Cannot resolve user home"))
}
fn default_root(home: &Path, data: Option<&Path>) -> io::Result<PathBuf> {
    let destination = home.join(".rune/tools");
    let visible = home.join("rune/tools");
    let mut sources = vec![visible.clone()];
    // A prior completed migration consumed the platform directory. Its retained
    // originals may be stale; never import them over the current visible tree.
    if !visible.join(".legacy-migration-complete").is_file()
        && let Some(data) = data
    {
        let legacy = data.join("rune-kit");
        if legacy != visible {
            sources.push(legacy);
        }
    }
    migrate_sources(&sources, &destination)?;
    std::fs::create_dir_all(&destination)?;
    Ok(destination)
}
fn marker(source: &Path, destination: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(source.to_string_lossy().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    destination.join(format!(".migration-{hash}.complete"))
}
fn migrate_sources(sources: &[PathBuf], destination: &Path) -> io::Result<()> {
    for path in [destination, destination.parent().unwrap_or(destination)] {
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(io::Error::other("Migration refuses symbolic links"));
        }
    }
    let mut planned = Vec::new();
    let mut active = Vec::new();
    for source in sources {
        if source == destination || !source.exists() || marker(source, destination).is_file() {
            continue;
        }
        if std::fs::symlink_metadata(source)?.file_type().is_symlink() {
            return Err(io::Error::other("Migration refuses symbolic links"));
        }
        for name in [
            "plugins",
            "skills",
            "_bin",
            "cache",
            "_cache",
            "installed.json",
            "installed_skills.json",
        ] {
            collect(&source.join(name), &destination.join(name), &mut planned)?;
        }
        active.push(source);
    }
    // Preflight every source and duplicate destination before publishing any file.
    let mut destinations = std::collections::HashMap::<PathBuf, PathBuf>::new();
    for (from, to) in &planned {
        if let Some(previous) = destinations.insert(to.clone(), from.clone())
            && std::fs::read(previous)? != std::fs::read(from)?
        {
            return Err(io::Error::other(format!(
                "Conflicting legacy copies for {}; originals retained",
                to.display()
            )));
        }
        if to.exists() && std::fs::read(from)? != std::fs::read(to)? {
            return Err(io::Error::other(format!(
                "Migration conflict at {}; original retained at {}",
                to.display(),
                from.display()
            )));
        }
    }
    for source in active {
        migrate(source, destination)?;
    }
    Ok(())
}

/// Copy only known managed entries; originals remain available for recovery.
/// Existing identical files are accepted. Conflicts fail without overwriting.
pub fn migrate(source: &Path, destination: &Path) -> io::Result<()> {
    if source == destination || marker(source, destination).is_file() || !source.exists() {
        return Ok(());
    }
    let mut files = Vec::new();
    for name in [
        "plugins",
        "skills",
        "_bin",
        "cache",
        "_cache",
        "installed.json",
        "installed_skills.json",
    ] {
        collect(&source.join(name), &destination.join(name), &mut files)?;
    }
    // Validate every conflict before making any changes.
    for (from, to) in &files {
        if to.exists() && std::fs::read(from)? != std::fs::read(to)? {
            return Err(io::Error::other(format!(
                "Migration conflict at {}; original retained at {}",
                to.display(),
                from.display()
            )));
        }
    }
    for (from, to) in files {
        if to.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Stage alongside the target; a crash cannot leave a partial payload at its final path.
        let stage = to.with_file_name(format!(
            ".rune-kit-migrate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let result = (|| -> io::Result<()> {
            let mut target = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&stage)?;
            let mut input = std::fs::File::open(&from)?;
            io::copy(&mut input, &mut target)?;
            target.sync_all()?;
            drop(target);
            std::fs::set_permissions(&stage, std::fs::metadata(&from)?.permissions())?;
            if std::fs::read(&from)? != std::fs::read(&stage)? {
                return Err(io::Error::other(
                    "Migration validation failed; original retained",
                ));
            }
            // Hard-link publication is atomic and fails if another writer created the target.
            std::fs::hard_link(&stage, &to)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&stage);
        result?;
    }
    std::fs::create_dir_all(destination)?;
    std::fs::write(
        marker(source, destination),
        b"Original legacy storage retained.\n",
    )?;
    Ok(())
}

fn collect(from: &Path, to: &Path, files: &mut Vec<(PathBuf, PathBuf)>) -> io::Result<()> {
    let meta = match std::fs::symlink_metadata(from) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink()
        || std::fs::symlink_metadata(to).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(io::Error::other("Migration refuses symbolic links"));
    }
    if meta.is_dir() {
        if to.exists() && !to.is_dir() {
            return Err(io::Error::other("Migration directory conflict"));
        }
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            collect(&entry.path(), &to.join(entry.file_name()), files)?;
        }
    } else if meta.is_file() {
        files.push((from.to_owned(), to.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_retains_originals_repeats_and_preserves_infer() {
        let root = std::env::temp_dir().join(format!("kit migration {}", std::process::id()));
        let old = root.join("old");
        let new = root.join("rune/tools");
        std::fs::create_dir_all(old.join("plugins")).unwrap();
        std::fs::create_dir_all(root.join("rune/infer")).unwrap();
        std::fs::write(old.join("plugins/test.wasm"), b"payload").unwrap();
        std::fs::write(root.join("rune/infer/config"), b"infer").unwrap();
        migrate(&old, &new).unwrap();
        migrate(&old, &new).unwrap();
        assert_eq!(
            std::fs::read(new.join("plugins/test.wasm")).unwrap(),
            b"payload"
        );
        assert!(old.join("plugins/test.wasm").exists());
        std::fs::write(new.join("plugins/test.wasm"), b"updated").unwrap();
        migrate(&old, &new).unwrap();
        assert_eq!(
            std::fs::read(new.join("plugins/test.wasm")).unwrap(),
            b"updated"
        );
        assert_eq!(
            std::fs::read(root.join("rune/infer/config")).unwrap(),
            b"infer"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn migration_resumes_validated_files_after_interruption() {
        let root = std::env::temp_dir().join(format!("kit interrupted {}", std::process::id()));
        let old = root.join("old");
        let new = root.join("new");
        std::fs::create_dir_all(old.join("plugins")).unwrap();
        std::fs::create_dir_all(new.join("plugins")).unwrap();
        std::fs::write(old.join("plugins/one"), b"one").unwrap();
        std::fs::write(old.join("plugins/two"), b"two").unwrap();
        std::fs::write(new.join("plugins/one"), b"one").unwrap();
        std::fs::write(
            new.join("plugins/.rune-kit-migrate-interrupted"),
            b"partial",
        )
        .unwrap();
        migrate(&old, &new).unwrap();
        assert_eq!(std::fs::read(new.join("plugins/two")).unwrap(), b"two");
        assert!(marker(&old, &new).exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn migration_conflict_never_overwrites() {
        let root = std::env::temp_dir().join(format!("kit conflict {}", std::process::id()));
        let old = root.join("old");
        let new = root.join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("installed.json"), b"old").unwrap();
        std::fs::write(new.join("installed.json"), b"different").unwrap();
        assert!(migrate(&old, &new).is_err());
        assert_eq!(
            std::fs::read(new.join("installed.json")).unwrap(),
            b"different"
        );
        assert_eq!(std::fs::read(old.join("installed.json")).unwrap(), b"old");
        std::fs::remove_file(new.join("installed.json")).unwrap();
        migrate(&old, &new).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod hidden_root_tests {
    use super::*;
    #[test]
    fn visible_tree_metadata_caches_and_external_paths_survive_repeated_migration() {
        let temp = std::env::temp_dir().join(format!("kit hidden home {}", std::process::id()));
        let old = temp.join("rune/tools");
        std::fs::create_dir_all(old.join("plugins")).unwrap();
        std::fs::create_dir_all(old.join("_bin/git/1")).unwrap();
        std::fs::create_dir_all(temp.join(".rune/infer/config")).unwrap();
        std::fs::write(old.join("plugins/plugin.wasm"), b"plugin").unwrap();
        std::fs::write(old.join("_bin/git/1/git"), b"cached helper").unwrap();
        let metadata=br#"{"plugins":{"p":{"binary_path":"plugins/plugin.wasm","default_params":{"project":"/external/projects","model":"/external/models/model.bin"}}}}"#;
        std::fs::write(old.join("installed.json"), metadata).unwrap();
        std::fs::write(temp.join(".rune/rune-infer"), b"infer").unwrap();
        std::fs::write(temp.join("rune/project.txt"), b"unrelated").unwrap();
        let new = default_root(&temp, None).unwrap();
        assert_eq!(new, temp.join(".rune/tools"));
        for _ in 0..2 {
            default_root(&temp, None).unwrap();
        }
        assert_eq!(std::fs::read(new.join("installed.json")).unwrap(), metadata);
        assert_eq!(
            std::fs::read(new.join("plugins/plugin.wasm")).unwrap(),
            b"plugin"
        );
        assert_eq!(
            std::fs::read(new.join("_bin/git/1/git")).unwrap(),
            b"cached helper"
        );
        assert_eq!(
            std::fs::read(temp.join(".rune/rune-infer")).unwrap(),
            b"infer"
        );
        assert_eq!(
            std::fs::read(temp.join("rune/project.txt")).unwrap(),
            b"unrelated"
        );
        assert!(old.join("plugins/plugin.wasm").exists());
        assert!(!new.join("project.txt").exists());
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn fresh_defaults_and_older_platform_sources_share_hidden_root() {
        let temp = std::env::temp_dir().join(format!("kit platform hidden {}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let data = temp.join("platform-data");
        let legacy = data.join("rune-kit");
        std::fs::create_dir_all(legacy.join("skills")).unwrap();
        std::fs::write(legacy.join("skills/readme.md"), b"skill").unwrap();
        let new = default_root(&temp, Some(&data)).unwrap();
        assert_eq!(
            std::fs::read(new.join("skills/readme.md")).unwrap(),
            b"skill"
        );
        assert!(legacy.join("skills/readme.md").exists());
        default_root(&temp, Some(&data)).unwrap();
        let fresh = temp.join("fresh home");
        std::fs::create_dir_all(&fresh).unwrap();
        assert_eq!(
            default_root(&fresh, None).unwrap(),
            fresh.join(".rune/tools")
        );
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn divergent_legacy_registries_fail_before_copying_and_completed_old_backups_are_not_reimported()
     {
        let temp = std::env::temp_dir().join(format!("kit multisource {}", std::process::id()));
        let old = temp.join("rune/tools");
        let data = temp.join("platform");
        let legacy = data.join("rune-kit");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(old.join("installed.json"), b"current registry").unwrap();
        std::fs::write(legacy.join("installed.json"), b"old registry").unwrap();
        assert!(default_root(&temp, Some(&data)).is_err());
        assert!(!temp.join(".rune/tools/installed.json").exists());
        // Prior releases explicitly left this marker after validating the platform copy.
        std::fs::write(
            old.join(".legacy-migration-complete"),
            b"validated original migration",
        )
        .unwrap();
        let new = default_root(&temp, Some(&data)).unwrap();
        assert_eq!(
            std::fs::read(new.join("installed.json")).unwrap(),
            b"current registry"
        );
        assert_eq!(
            std::fs::read(legacy.join("installed.json")).unwrap(),
            b"old registry"
        );
        std::fs::remove_dir_all(temp).unwrap();
    }
}
