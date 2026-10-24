//! Workspace and approval inventory only. These records do not grant runtime access,
//! isolate writes, copy files, or authorize copying changes back to originals.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub path: PathBuf,
    pub kind: PathKind,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema_version: u32,
    plugins: BTreeMap<String, Vec<Approval>>,
}

pub struct Sandbox {
    root: PathBuf,
    inventory: Inventory,
    original: Option<Vec<u8>>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

pub(crate) fn check_links(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if metadata.file_type().is_symlink() || reparse {
                    return Err(invalid(
                        "Sandbox storage refuses symbolic links and junctions",
                    ));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn directory(path: &Path) -> io::Result<()> {
    check_links(path)?;
    fs::create_dir_all(path)?;
    check_links(path)
}

fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    check_links(path)?;
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Sandbox {
    pub fn open(root: PathBuf) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(invalid("Sandbox root must be absolute"));
        }
        directory(&root)?;
        let root = fs::canonicalize(root)?;
        let original = read(&root.join("approvals.json"))?;
        let inventory: Inventory = original
            .as_ref()
            .map(|bytes| serde_json::from_slice(bytes))
            .transpose()
            .map_err(|_| invalid("Invalid sandbox approvals; existing data retained"))?
            .unwrap_or(Inventory {
                schema_version: 1,
                plugins: BTreeMap::new(),
            });
        if inventory.schema_version != 1
            || inventory.plugins.iter().any(|(name, approvals)| {
                name.is_empty()
                    || approvals
                        .iter()
                        .any(|a| !a.path.is_absolute() || a.path.starts_with(&root))
            })
        {
            return Err(invalid("Unsupported or invalid sandbox approval inventory"));
        }
        Ok(Self {
            root,
            inventory,
            original,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Plugin identifiers never become path components, including local artifact paths.
    pub fn workspace(&self, plugin: &str) -> io::Result<PathBuf> {
        if plugin.is_empty() {
            return Err(invalid("Select a plugin first"));
        }
        let key: String = Sha256::digest(plugin.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self.root.join("plugins").join(key);
        directory(&path)?;
        Ok(path)
    }

    pub fn approvals(&self, plugin: &str) -> &[Approval] {
        self.inventory
            .plugins
            .get(plugin)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Records intent only; does not read file contents or change plugin permissions.
    pub fn approve(&mut self, plugin: &str, path: &Path) -> io::Result<()> {
        if plugin.is_empty() || !path.is_absolute() {
            return Err(invalid(
                "Select a plugin and an existing absolute file or folder path",
            ));
        }
        let path = fs::canonicalize(path)?;
        if path.starts_with(&self.root) {
            return Err(invalid("Select an external path, outside the sandbox"));
        }
        let metadata = fs::metadata(&path)?;
        let kind = if metadata.is_file() {
            PathKind::File
        } else if metadata.is_dir() {
            PathKind::Directory
        } else {
            return Err(invalid("Only regular files and folders can be approved"));
        };
        let mut next = self.inventory.clone();
        let approvals = next.plugins.entry(plugin.to_owned()).or_default();
        if approvals.iter().any(|a| a.path == path) {
            return self.save(next);
        }
        approvals.push(Approval { path, kind });
        approvals.sort_by(|a, b| a.path.cmp(&b.path));
        self.save(next)
    }

    /// Revoking approval never removes the workspace or any external data.
    pub fn revoke(&mut self, plugin: &str, path: &Path) -> io::Result<()> {
        let mut next = self.inventory.clone();
        if let Some(approvals) = next.plugins.get_mut(plugin) {
            approvals.retain(|a| a.path != path);
        }
        self.save(next)
    }

    fn save(&mut self, next: Inventory) -> io::Result<()> {
        check_links(&self.root)?;
        let lock = self.root.join("approvals.lock");
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|_| invalid("Approval storage is locked; another save may be active"))?;
        let _lock = Lock(lock);
        drop(file);
        let path = self.root.join("approvals.json");
        if read(&path)? != self.original {
            return Err(invalid(
                "Approvals changed outside this dashboard; reopen the sandbox panel",
            ));
        }
        let bytes = serde_json::to_vec_pretty(&next).map_err(io::Error::other)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let stage = self
            .root
            .join(format!(".approvals-{}-{nonce}.tmp", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&stage)?;
        let result = (|| -> io::Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&stage, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(stage);
        }
        result?;
        self.inventory = next;
        self.original = Some(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("kit-sandbox-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn sandbox(&self) -> Sandbox {
            Sandbox::open(self.0.join("sandbox")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn approvals_roundtrip_without_copying_or_changing_originals_and_revoke_retains_work() {
        let fixture = Fixture::new();
        let file = fixture.0.join("original.txt");
        let folder = fixture.0.join("external-folder");
        fs::write(&file, b"original data").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("child"), b"untouched").unwrap();
        let mut sandbox = fixture.sandbox();
        let workspace = sandbox.workspace("rune-time").unwrap();
        sandbox.approve("rune-time", &file).unwrap();
        sandbox.approve("rune-time", &folder).unwrap();
        sandbox.approve("rune-time", &file).unwrap();
        assert_eq!(sandbox.approvals("rune-time").len(), 2);
        assert!(sandbox.approvals("another-plugin").is_empty());
        assert_eq!(fs::read_dir(&workspace).unwrap().count(), 0);
        let restored = fixture.sandbox();
        assert_eq!(
            restored.approvals("rune-time"),
            sandbox.approvals("rune-time")
        );
        assert!(
            restored
                .approvals("rune-time")
                .iter()
                .any(|a| a.kind == PathKind::File)
        );
        assert!(
            restored
                .approvals("rune-time")
                .iter()
                .any(|a| a.kind == PathKind::Directory)
        );
        fs::write(workspace.join("pending-work"), b"keep").unwrap();
        sandbox
            .revoke("rune-time", &fs::canonicalize(&file).unwrap())
            .unwrap();
        assert_eq!(sandbox.approvals("rune-time").len(), 1);
        assert_eq!(fs::read(&file).unwrap(), b"original data");
        assert_eq!(fs::read(folder.join("child")).unwrap(), b"untouched");
        assert_eq!(fs::read(workspace.join("pending-work")).unwrap(), b"keep");
        assert_eq!(fixture.sandbox().approvals("rune-time").len(), 1);
    }

    #[test]
    fn workspaces_stay_inside_root_for_local_paths_and_traversal_identifiers() {
        let fixture = Fixture::new();
        let sandbox = fixture.sandbox();
        for name in [
            "../../escape",
            "C:\\plugins\\native.exe",
            "CON",
            "rune-time",
        ] {
            let workspace = sandbox.workspace(name).unwrap();
            assert!(workspace.starts_with(sandbox.root()));
            assert_eq!(workspace.parent().unwrap(), sandbox.root().join("plugins"));
            assert_eq!(workspace, sandbox.workspace(name).unwrap());
        }
        assert_ne!(
            sandbox.workspace("one").unwrap(),
            sandbox.workspace("two").unwrap()
        );
        assert!(sandbox.workspace("").is_err());
    }

    #[test]
    fn rejects_relative_missing_and_internal_paths_and_preserves_corrupt_inventory() {
        let fixture = Fixture::new();
        assert!(Sandbox::open(PathBuf::from("relative")).is_err());
        let mut sandbox = fixture.sandbox();
        assert!(sandbox.approve("test", Path::new("relative")).is_err());
        assert!(sandbox.approve("test", &fixture.0.join("missing")).is_err());
        let workspace = sandbox.workspace("test").unwrap();
        assert!(sandbox.approve("test", &workspace).is_err());
        let inventory = sandbox.root().join("approvals.json");
        fs::write(&inventory, b"broken inventory").unwrap();
        assert!(Sandbox::open(sandbox.root().to_owned()).is_err());
        assert_eq!(fs::read(&inventory).unwrap(), b"broken inventory");
    }

    #[test]
    fn stale_dashboard_and_locked_storage_cannot_overwrite_saved_approvals() {
        let fixture = Fixture::new();
        let file = fixture.0.join("file");
        fs::write(&file, b"data").unwrap();
        let mut first = fixture.sandbox();
        let mut stale = fixture.sandbox();
        first.approve("first", &file).unwrap();
        let saved = fs::read(first.root().join("approvals.json")).unwrap();
        assert!(stale.approve("second", &file).is_err());
        assert!(stale.approvals("second").is_empty());
        assert_eq!(
            fs::read(first.root().join("approvals.json")).unwrap(),
            saved
        );
        let lock = first.root().join("approvals.lock");
        fs::write(&lock, b"another writer").unwrap();
        assert!(
            first
                .revoke("first", &fs::canonicalize(&file).unwrap())
                .is_err()
        );
        assert_eq!(first.approvals("first").len(), 1);
        assert_eq!(fs::read(lock).unwrap(), b"another writer");
    }

    #[cfg(unix)]
    #[test]
    fn storage_links_cannot_redirect_workspace_or_inventory_writes() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, fixture.0.join("linked-root")).unwrap();
        assert!(Sandbox::open(fixture.0.join("linked-root")).is_err());
        let sandbox = fixture.sandbox();
        symlink(&outside, sandbox.root().join("plugins")).unwrap();
        assert!(sandbox.workspace("test").is_err());
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }
}
