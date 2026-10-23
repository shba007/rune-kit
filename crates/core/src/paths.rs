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
    let destination = home.join("rune/tools");
    if let Some(data) = dirs::data_dir() {
        migrate(&data.join("rune-kit"), &destination)?;
    }
    Ok(destination)
}

/// Copy only known managed entries; originals remain available for recovery.
/// Existing identical files are accepted. Conflicts fail without overwriting.
pub fn migrate(source: &Path, destination: &Path) -> io::Result<()> {
    if source == destination
        || destination.join(".legacy-migration-complete").is_file()
        || !source.exists()
    {
        return Ok(());
    }
    let mut files = Vec::new();
    for name in [
        "plugins",
        "skills",
        "_bin",
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
        destination.join(".legacy-migration-complete"),
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
        assert!(new.join(".legacy-migration-complete").exists());
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
