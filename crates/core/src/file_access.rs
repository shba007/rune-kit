//! Version 1 of the cooperative plugin file-access API. Approval and copy-back
//! are host-only operations; guest requests can never authorize original writes.
use crate::sandbox::{PathKind, Sandbox, check_links};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const LIMIT: u64 = 16 * 1024 * 1024;
fn error(message: &str) -> io::Error {
    io::Error::other(message)
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn now() -> io::Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs())
}
fn transaction_id() -> io::Result<String> {
    Ok(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos()
    ))
}
fn bytes(path: &Path) -> io::Result<Vec<u8>> {
    check_links(path)?;
    if fs::metadata(path)?.len() > LIMIT {
        return Err(error("File exceeds the 16 MiB broker limit"));
    }
    let mut data = Vec::new();
    fs::File::open(path)?
        .take(LIMIT + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > LIMIT {
        return Err(error("File exceeds the broker limit"));
    }
    Ok(data)
}
fn fingerprint(path: &Path) -> io::Result<Option<String>> {
    match bytes(path) {
        Ok(data) => Ok(Some(hash(&data))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    check_links(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| error("Missing destination directory"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let stage = parent.join(format!(".rune-write-{}-{nonce}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&stage)?;
    let result = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&stage, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(stage);
    }
    result
}
fn json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    atomic(
        path,
        &serde_json::to_vec_pretty(value).map_err(io::Error::other)?,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub operation: Operation,
    pub path: PathBuf,
    #[serde(default)]
    pub data: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    ReadFile,
    WriteFile,
    DeleteFile,
    ListDirectory,
}
#[derive(Serialize)]
pub struct Response {
    pub schema_version: u32,
    pub staged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<String>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    id: String,
    path: PathBuf,
    baseline: Option<String>,
    deleted: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observed {
    path: PathBuf,
    digest: String,
}
#[derive(Clone, Serialize)]
pub struct Review {
    pub path: PathBuf,
    pub staged_path: PathBuf,
    pub deleted: bool,
    pub baseline: Option<String>,
    pub staged_digest: Option<String>,
}
impl Review {
    pub fn approval_token(&self) -> io::Result<String> {
        Ok(hash(&serde_json::to_vec(self).map_err(io::Error::other)?))
    }
}

#[derive(Clone, Serialize)]
pub struct RecoveryInfo {
    pub path: PathBuf,
    pub id: String,
    pub backup_path: PathBuf,
    pub completed: Option<u64>,
}
impl RecoveryInfo {
    pub fn approval_token(&self) -> io::Result<String> {
        Ok(hash(&serde_json::to_vec(self).map_err(io::Error::other)?))
    }
}
#[derive(Serialize, Deserialize)]
struct Recovery {
    change: Change,
    staged_digest: Option<String>,
    completed: Option<u64>,
}

pub struct Broker {
    root: PathBuf,
    plugin: String,
    private: PathBuf,
}
struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Broker {
    pub fn open(root: PathBuf, plugin: &str) -> io::Result<Self> {
        let sandbox = Sandbox::open(root)?;
        let private = sandbox
            .root()
            .join("transactions")
            .join(hash(plugin.as_bytes()));
        check_links(&private)?;
        fs::create_dir_all(&private)?;
        check_links(&private)?;
        Ok(Self {
            root: sandbox.root().to_owned(),
            plugin: plugin.into(),
            private,
        })
    }
    fn lock(&self) -> io::Result<Lock> {
        check_links(&self.private)?;
        let path = self.private.join("transaction.lock");
        let _file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| error("Another file transaction is active"))?;
        Ok(Lock(path))
    }
    fn key(path: &Path) -> String {
        hash(path.to_string_lossy().as_bytes())
    }
    fn change(&self, path: &Path) -> PathBuf {
        self.private.join(format!("{}.change", Self::key(path)))
    }
    fn staged(&self, path: &Path) -> PathBuf {
        self.private.join(format!("{}.data", Self::key(path)))
    }
    fn authorize(&self, path: &Path) -> io::Result<PathBuf> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(error("Broker paths must be absolute without traversal"));
        }
        check_links(path)?;
        let resolved = match fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let parent = fs::canonicalize(
                    path.parent()
                        .ok_or_else(|| error("Missing parent directory"))?,
                )?;
                parent.join(path.file_name().ok_or_else(|| error("Missing filename"))?)
            }
            Err(e) => return Err(e),
        };
        let sandbox = Sandbox::open(self.root.clone())?;
        if resolved.starts_with(sandbox.root()) {
            return Err(error(
                "Broker requests must target an approved external path",
            ));
        }
        for approval in sandbox.approvals(&self.plugin) {
            check_links(&approval.path)?;
            let approved = match fs::canonicalize(&approval.path) {
                Ok(path) => path,
                Err(e)
                    if e.kind() == io::ErrorKind::NotFound && approval.kind == PathKind::File =>
                {
                    if resolved == approval.path {
                        return Ok(resolved);
                    }
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let metadata = fs::metadata(&approved)?;
            if (approval.kind == PathKind::File && !metadata.is_file())
                || (approval.kind == PathKind::Directory && !metadata.is_dir())
            {
                continue;
            }
            if (approval.kind == PathKind::File && resolved == approved)
                || (approval.kind == PathKind::Directory && resolved.starts_with(&approved))
            {
                return Ok(resolved);
            }
        }
        Err(error("External path has not been approved for this plugin"))
    }
    fn current(&self, path: &Path) -> io::Result<Option<Change>> {
        let record = self.change(path);
        check_links(&record)?;
        match fs::read(record) {
            Ok(data) => {
                let change: Change = serde_json::from_slice(&data).map_err(io::Error::other)?;
                if change.path != path
                    || change.id.is_empty()
                    || !change.id.chars().all(|c| c.is_ascii_digit() || c == '-')
                {
                    return Err(error("Invalid staged change identity"));
                }
                Ok(Some(change))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn request(&self, request: Request) -> io::Result<Response> {
        if request.schema_version != 1 {
            return Err(error("Unsupported file-access schema_version"));
        }
        if request.data.len() as u64 > LIMIT {
            return Err(error("Write exceeds the broker limit"));
        }
        let _lock = self.lock()?;
        let path = self.authorize(&request.path)?;
        let change = self.current(&path)?;
        match request.operation {
            Operation::ReadFile => {
                if change.as_ref().is_some_and(|c| c.deleted) {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "File is deleted in staged work",
                    ));
                }
                let source = if change.is_some() {
                    self.staged(&path)
                } else {
                    path.clone()
                };
                let data = bytes(&source)?;
                if change.is_none() {
                    json(
                        &self.private.join(format!("{}.observed", Self::key(&path))),
                        &Observed {
                            path: path.clone(),
                            digest: hash(&data),
                        },
                    )?;
                }
                Ok(Response {
                    schema_version: 1,
                    staged: change.is_some(),
                    data: Some(data),
                    entries: None,
                })
            }
            Operation::WriteFile | Operation::DeleteFile => {
                if self
                    .recoveries_unlocked()?
                    .iter()
                    .any(|(_, r)| r.change.path == path && r.completed.is_none())
                {
                    return Err(error(
                        "Resolve the previous copy-back recovery before editing this file",
                    ));
                }
                let deleted = matches!(request.operation, Operation::DeleteFile);
                let baseline = match &change {
                    Some(c) => c.baseline.clone(),
                    None => {
                        let observed = self.private.join(format!("{}.observed", Self::key(&path)));
                        match bytes(&observed) {
                            Ok(data) => {
                                let observed: Observed =
                                    serde_json::from_slice(&data).map_err(io::Error::other)?;
                                if observed.path != path {
                                    return Err(error("Invalid observed file identity"));
                                }
                                Some(observed.digest)
                            }
                            Err(e) if e.kind() == io::ErrorKind::NotFound => fingerprint(&path)?,
                            Err(e) => return Err(e),
                        }
                    }
                };
                if !deleted {
                    atomic(&self.staged(&path), &request.data)?;
                }
                json(
                    &self.change(&path),
                    &Change {
                        id: match &change {
                            Some(change) => change.id.clone(),
                            None => transaction_id()?,
                        },
                        path,
                        baseline,
                        deleted,
                    },
                )?;
                Ok(Response {
                    schema_version: 1,
                    staged: true,
                    data: None,
                    entries: None,
                })
            }
            Operation::ListDirectory => {
                let mut entries = fs::read_dir(&path)?
                    .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
                    .collect::<io::Result<Vec<_>>>()?;
                for review in self.reviews_unlocked()? {
                    if review.path.parent() == Some(path.as_path()) {
                        let name = review
                            .path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .into_owned();
                        entries.retain(|n| n != &name);
                        if !review.deleted {
                            entries.push(name);
                        }
                    }
                }
                entries.sort();
                Ok(Response {
                    schema_version: 1,
                    staged: false,
                    data: None,
                    entries: Some(entries),
                })
            }
        }
    }
    fn reviews_unlocked(&self) -> io::Result<Vec<Review>> {
        let mut reviews = Vec::new();
        for entry in fs::read_dir(&self.private)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "change") {
                continue;
            }
            let change: Change =
                serde_json::from_slice(&bytes(&path)?).map_err(io::Error::other)?;
            if self.change(&change.path) != path {
                return Err(error("Invalid staged change identity"));
            }
            let staged_path = self.staged(&change.path);
            let staged_digest = if change.deleted {
                None
            } else {
                fingerprint(&staged_path)?
            };
            if !change.deleted && staged_digest.is_none() {
                return Err(error("Staged content is missing"));
            }
            reviews.push(Review {
                path: change.path,
                staged_path,
                deleted: change.deleted,
                baseline: change.baseline,
                staged_digest,
            });
        }
        reviews.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(reviews)
    }
    pub fn reviews(&self) -> io::Result<Vec<Review>> {
        let _lock = self.lock()?;
        self.reviews_unlocked()
    }
    pub fn review_for(&self, path: &Path) -> io::Result<Review> {
        let path = self.authorize(path)?;
        self.reviews()?
            .into_iter()
            .find(|review| review.path == path)
            .ok_or_else(|| error("No staged changes for this path"))
    }

    /// Host-only. The caller must present this review and receive user approval.
    pub fn apply_review(&self, review: &Review) -> io::Result<()> {
        let _lock = self.lock()?;
        let path = self.authorize(&review.path)?;
        let change = self
            .current(&path)?
            .ok_or_else(|| error("Staged change no longer exists"))?;
        if change.baseline != review.baseline
            || change.deleted != review.deleted
            || fingerprint(&path)? != review.baseline
        {
            return Err(error(
                "Original changed; resolve the conflict before copy-back",
            ));
        }
        let staged = if change.deleted {
            None
        } else {
            Some(bytes(&self.staged(&path))?)
        };
        if staged.as_ref().map(|b| hash(b)) != review.staged_digest {
            return Err(error("Staged content changed; review it again"));
        }
        if change.id.is_empty() || !change.id.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return Err(error("Invalid transaction identity"));
        }
        let key = format!("{}-{}", Self::key(&path), change.id);
        let receipt = self.private.join(format!("{key}.recovery"));
        if receipt.exists() {
            return Err(error("Resolve the existing recovery record first"));
        }
        let backup = self.private.join(format!("{key}.backup"));
        if change.baseline.is_some() {
            atomic(&backup, &bytes(&path)?)?;
        }
        let mut recovery = Recovery {
            change: change.clone(),
            staged_digest: review.staged_digest.clone(),
            completed: None,
        };
        json(&receipt, &recovery)?;
        if fingerprint(&path)? != review.baseline {
            return Err(error(
                "Original changed during preparation; backup retained",
            ));
        }
        if let Some(data) = staged {
            atomic(&path, &data)?;
        } else if path.exists() {
            fs::remove_file(&path)?;
        }
        recovery.completed = Some(now()?);
        json(&receipt, &recovery)?;
        fs::remove_file(self.change(&path))?;
        if self.staged(&path).exists() {
            fs::remove_file(self.staged(&path))?;
        }
        let observed = self.private.join(format!("{}.observed", Self::key(&path)));
        if observed.exists() {
            fs::remove_file(observed)?;
        }
        Ok(())
    }

    fn recoveries_unlocked(&self) -> io::Result<Vec<(PathBuf, Recovery)>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.private)? {
            let receipt = entry?.path();
            if receipt.extension().is_none_or(|e| e != "recovery") {
                continue;
            }
            let recovery: Recovery =
                serde_json::from_slice(&bytes(&receipt)?).map_err(io::Error::other)?;
            if recovery.change.id.is_empty()
                || !recovery
                    .change
                    .id
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '-')
                || receipt
                    != self.private.join(format!(
                        "{}-{}.recovery",
                        Self::key(&recovery.change.path),
                        recovery.change.id
                    ))
            {
                return Err(error("Invalid recovery identity"));
            }
            records.push((receipt, recovery));
        }
        Ok(records)
    }

    /// Host-only recovery. Original edits made after copy-back require resolution.
    pub fn restore(&self, info: &RecoveryInfo) -> io::Result<()> {
        let _lock = self.lock()?;
        let path = self.authorize(&info.path)?;
        let (receipt, mut recovery) = self
            .recoveries_unlocked()?
            .into_iter()
            .find(|(_, r)| {
                r.change.path == path && r.change.id == info.id && r.completed == info.completed
            })
            .ok_or_else(|| error("No recovery backup exists"))?;
        let current = fingerprint(&path)?;
        if current != recovery.staged_digest && current != recovery.change.baseline {
            return Err(error(
                "Original changed after copy-back; backup retained for manual resolution",
            ));
        }
        if recovery.change.baseline.is_some() {
            let backup = bytes(&receipt.with_extension("backup"))?;
            if Some(hash(&backup)) != recovery.change.baseline {
                return Err(error("Recovery backup integrity check failed"));
            }
            atomic(&path, &backup)?;
        } else if path.exists() {
            fs::remove_file(&path)?;
        }
        recovery.completed = Some(now()?);
        json(&receipt, &recovery)?;
        if let Some(mut pending) = self.current(&path)?
            && pending.id == recovery.change.id
        {
            pending.id = transaction_id()?;
            pending.baseline = fingerprint(&path)?;
            json(&self.change(&path), &pending)?;
        }
        Ok(())
    }

    pub fn recoveries(&self) -> io::Result<Vec<RecoveryInfo>> {
        let _lock = self.lock()?;
        Ok(self
            .recoveries_unlocked()?
            .into_iter()
            .map(|(receipt, r)| RecoveryInfo {
                path: r.change.path,
                id: r.change.id,
                backup_path: receipt.with_extension("backup"),
                completed: r.completed,
            })
            .collect())
    }

    /// Completed recovery backups expire after seven days; unresolved ones never do.
    pub fn prune_completed(&self) -> io::Result<usize> {
        let _lock = self.lock()?;
        let mut count = 0;
        let time = now()?;
        for (receipt, recovery) in self.recoveries_unlocked()? {
            if recovery
                .completed
                .is_some_and(|done| time.saturating_sub(done) >= 7 * 24 * 60 * 60)
            {
                let backup = receipt.with_extension("backup");
                if backup.exists() {
                    check_links(&backup)?;
                    fs::remove_file(backup)?;
                }
                fs::remove_file(receipt)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        file: PathBuf,
        broker: Broker,
    }
    impl Fixture {
        fn new(folder: bool) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("kit-broker-{}-{nonce}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let external = root.join("external");
            fs::create_dir(&external).unwrap();
            let file = external.join("original.txt");
            fs::write(&file, b"original").unwrap();
            let mut sandbox = Sandbox::open(root.join("sandbox")).unwrap();
            sandbox
                .approve("fixture", if folder { &external } else { &file })
                .unwrap();
            let file = fs::canonicalize(file).unwrap();
            let broker = Broker::open(root.join("sandbox"), "fixture").unwrap();
            Self { root, file, broker }
        }
        fn request(&self, operation: Operation, path: &Path, data: &[u8]) -> io::Result<Response> {
            self.broker.request(Request {
                schema_version: 1,
                operation,
                path: path.to_owned(),
                data: data.to_vec(),
            })
        }
        fn write(&self, data: &[u8]) {
            self.request(Operation::WriteFile, &self.file, data)
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn reads_originals_without_copying_and_only_stages_changed_files() {
        let f = Fixture::new(true);
        let read = f.request(Operation::ReadFile, &f.file, &[]).unwrap();
        assert_eq!(read.data.unwrap(), b"original");
        assert!(!read.staged);
        assert!(fs::read_dir(&f.broker.private).unwrap().all(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "observed")
        }));
        f.write(b"changed");
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
        assert_eq!(
            f.request(Operation::ReadFile, &f.file, &[])
                .unwrap()
                .data
                .unwrap(),
            b"changed"
        );
        let added = f.file.parent().unwrap().join("added.txt");
        f.request(Operation::WriteFile, &added, b"new").unwrap();
        assert!(!added.exists());
        f.request(Operation::DeleteFile, &f.file, &[]).unwrap();
        assert!(f.request(Operation::ReadFile, &f.file, &[]).is_err());
        let entries = f
            .request(Operation::ListDirectory, f.file.parent().unwrap(), &[])
            .unwrap()
            .entries
            .unwrap();
        assert_eq!(entries, vec!["added.txt"]);
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
    }

    #[test]
    fn rejects_stale_reviews_and_external_conflicts_before_touching_originals() {
        let f = Fixture::new(false);
        f.write(b"first");
        let stale = f.broker.reviews().unwrap().remove(0);
        f.write(b"second");
        assert!(f.broker.apply_review(&stale).is_err());
        let review = f.broker.reviews().unwrap().remove(0);
        assert_ne!(
            review.approval_token().unwrap(),
            stale.approval_token().unwrap()
        );
        fs::write(&f.file, b"outside change").unwrap();
        assert!(f.broker.apply_review(&review).is_err());
        assert_eq!(fs::read(&f.file).unwrap(), b"outside change");
        assert!(f.broker.recoveries().unwrap().is_empty());
        assert_eq!(f.broker.reviews().unwrap().len(), 1);
    }

    #[test]
    fn original_changes_between_read_and_first_write_are_not_silently_accepted() {
        let f = Fixture::new(false);
        f.request(Operation::ReadFile, &f.file, &[]).unwrap();
        fs::write(&f.file, b"external update after plugin read").unwrap();
        f.write(b"plugin update based on old data");
        assert!(
            f.broker
                .apply_review(&f.broker.reviews().unwrap().remove(0))
                .is_err()
        );
        assert_eq!(
            fs::read(&f.file).unwrap(),
            b"external update after plugin read"
        );
    }

    #[test]
    fn approved_copy_back_and_exact_backup_restore_work_across_repeated_edits() {
        let f = Fixture::new(false);
        for value in [b"first".as_slice(), b"second".as_slice()] {
            f.write(value);
            let review = f.broker.reviews().unwrap().remove(0);
            f.broker.apply_review(&review).unwrap();
            assert_eq!(fs::read(&f.file).unwrap(), value);
            assert!(f.broker.reviews().unwrap().is_empty());
        }
        let records = f.broker.recoveries().unwrap();
        assert_eq!(records.len(), 2);
        let latest = records
            .iter()
            .find(|info| fs::read(&info.backup_path).unwrap() == b"first")
            .unwrap();
        f.broker.restore(latest).unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"first");
        let original = records
            .iter()
            .find(|info| fs::read(&info.backup_path).unwrap() == b"original")
            .unwrap();
        f.broker.restore(original).unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
    }

    #[test]
    fn deleted_exact_file_and_new_file_can_be_recovered_without_parent_approval() {
        let f = Fixture::new(false);
        f.request(Operation::DeleteFile, &f.file, &[]).unwrap();
        f.broker
            .apply_review(&f.broker.reviews().unwrap().remove(0))
            .unwrap();
        assert!(!f.file.exists());
        f.broker
            .restore(&f.broker.recoveries().unwrap().remove(0))
            .unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
        let folder = Fixture::new(true);
        let new = folder.file.parent().unwrap().join("new.txt");
        folder.request(Operation::WriteFile, &new, b"new").unwrap();
        folder
            .broker
            .apply_review(&folder.broker.reviews().unwrap().remove(0))
            .unwrap();
        assert_eq!(fs::read(&new).unwrap(), b"new");
        folder
            .broker
            .restore(&folder.broker.recoveries().unwrap().remove(0))
            .unwrap();
        assert!(!new.exists());
    }

    #[test]
    fn recovery_does_not_overwrite_new_external_edits_or_corrupt_backups() {
        let f = Fixture::new(false);
        f.write(b"new");
        f.broker
            .apply_review(&f.broker.reviews().unwrap().remove(0))
            .unwrap();
        let info = f.broker.recoveries().unwrap().remove(0);
        fs::write(&f.file, b"external newer").unwrap();
        assert!(f.broker.restore(&info).is_err());
        assert_eq!(fs::read(&f.file).unwrap(), b"external newer");
        fs::write(&f.file, b"new").unwrap();
        fs::write(&info.backup_path, b"corrupt").unwrap();
        assert!(f.broker.restore(&info).is_err());
        assert_eq!(fs::read(&f.file).unwrap(), b"new");
    }

    #[test]
    fn retention_keeps_unresolved_backups_and_all_unapproved_work() {
        let f = Fixture::new(false);
        f.write(b"new");
        f.broker
            .apply_review(&f.broker.reviews().unwrap().remove(0))
            .unwrap();
        let (receipt, mut record) = f.broker.recoveries_unlocked().unwrap().remove(0);
        record.completed = None;
        json(&receipt, &record).unwrap();
        assert_eq!(f.broker.prune_completed().unwrap(), 0);
        assert!(
            f.request(Operation::WriteFile, &f.file, b"blocked")
                .is_err()
        );
        record.completed = Some(0);
        json(&receipt, &record).unwrap();
        f.write(b"pending");
        assert_eq!(f.broker.prune_completed().unwrap(), 1);
        assert_eq!(f.broker.reviews().unwrap().len(), 1);
        assert_eq!(
            f.request(Operation::ReadFile, &f.file, &[])
                .unwrap()
                .data
                .unwrap(),
            b"pending"
        );
        assert_eq!(fs::read(&f.file).unwrap(), b"new");
    }

    #[test]
    fn interrupted_copy_back_can_be_restored_and_pending_work_remains_applicable() {
        let f = Fixture::new(false);
        f.write(b"pending");
        let change = f.broker.current(&f.file).unwrap().unwrap();
        let review = f.broker.reviews().unwrap().remove(0);
        let receipt =
            f.broker
                .private
                .join(format!("{}-{}.recovery", Broker::key(&f.file), change.id));
        atomic(&receipt.with_extension("backup"), b"original").unwrap();
        json(
            &receipt,
            &Recovery {
                change: change.clone(),
                staged_digest: review.staged_digest,
                completed: None,
            },
        )
        .unwrap();
        fs::write(&f.file, b"pending").unwrap();
        f.broker
            .restore(&f.broker.recoveries().unwrap().remove(0))
            .unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
        assert_ne!(f.broker.current(&f.file).unwrap().unwrap().id, change.id);
        f.broker
            .apply_review(&f.broker.reviews().unwrap().remove(0))
            .unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"pending");
    }

    #[test]
    fn approval_scope_revocation_locking_and_guest_operation_validation_are_enforced() {
        let f = Fixture::new(false);
        let other = f.file.parent().unwrap().join("other.txt");
        fs::write(&other, b"private").unwrap();
        assert!(f.request(Operation::ReadFile, &other, &[]).is_err());
        assert!(
            f.request(Operation::ReadFile, Path::new("relative"), &[])
                .is_err()
        );
        assert!(
            f.request(
                Operation::ReadFile,
                &f.root.join("sandbox/approvals.json"),
                &[]
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Request>(
                serde_json::json!({"schema_version":1,"operation":"apply_review","path":f.file})
            )
            .is_err()
        );
        let lock = f.broker.private.join("transaction.lock");
        fs::write(&lock, b"owned by another writer").unwrap();
        assert!(f.request(Operation::ReadFile, &f.file, &[]).is_err());
        assert!(lock.exists());
        fs::remove_file(lock).unwrap();
        let mut sandbox = Sandbox::open(f.root.join("sandbox")).unwrap();
        sandbox.revoke("fixture", &f.file).unwrap();
        assert!(f.request(Operation::ReadFile, &f.file, &[]).is_err());
    }
}
