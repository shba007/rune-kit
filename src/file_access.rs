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
const DIRECTORY: &str = "directory";
fn fingerprint(path: &Path) -> io::Result<Option<String>> {
    check_links(path)?;
    match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => return Ok(Some(DIRECTORY.into())),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
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
    Stat,
    CreateDirectory,
    RemoveDirectory,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    #[default]
    File,
    Directory,
}
#[derive(Serialize)]
pub struct Stat {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ChangeKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
}
#[derive(Serialize)]
pub struct Response {
    pub schema_version: u32,
    pub staged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stat: Option<Stat>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    id: String,
    path: PathBuf,
    baseline: Option<String>,
    deleted: bool,
    #[serde(default)]
    kind: ChangeKind,
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
    pub kind: ChangeKind,
}
impl Review {
    pub fn approval_token(&self) -> io::Result<String> {
        Ok(hash(&serde_json::to_vec(self).map_err(io::Error::other)?))
    }
}

#[derive(Clone, Serialize)]
pub struct RecoveryInfo {
    pub kind: ChangeKind,
    pub path: PathBuf,
    pub id: String,
    pub backup_path: PathBuf,
    pub completed: Option<u64>,
}

#[derive(Serialize)]
pub struct ChangesView {
    pub reviews: Vec<Review>,
    pub recoveries: Vec<RecoveryInfo>,
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
    pub fn apply_approved(
        root: PathBuf,
        plugin: &str,
        action: &str,
        approval: &str,
    ) -> io::Result<()> {
        let inventory = Self::inspect(root.clone(), plugin)?;
        match action {
            "apply" => {
                for review in inventory.reviews {
                    if review.approval_token()? == approval {
                        return Self::open(root, plugin)?.apply_review(&review);
                    }
                }
            }
            "restore" => {
                for recovery in inventory.recoveries {
                    if recovery.approval_token()? == approval {
                        return Self::open(root, plugin)?.restore(&recovery);
                    }
                }
            }
            _ => return Err(error("Unknown review action")),
        }
        Err(error("The reviewed item changed or no longer exists"))
    }

    pub fn inspect(root: PathBuf, plugin: &str) -> io::Result<ChangesView> {
        Sandbox::inspect(root.clone(), &[plugin.to_owned()])?;
        if !root.try_exists()? {
            return Ok(ChangesView {
                reviews: vec![],
                recoveries: vec![],
            });
        }
        let root = fs::canonicalize(root)?;
        let private = root.join("transactions").join(hash(plugin.as_bytes()));
        check_links(&private)?;
        match fs::metadata(&private) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(error("Transaction storage is not a directory")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(ChangesView {
                    reviews: vec![],
                    recoveries: vec![],
                });
            }
            Err(e) => return Err(e),
        }
        let broker = Self {
            root,
            plugin: plugin.to_owned(),
            private,
        };
        let _lock = broker.lock()?;
        let reviews = broker.reviews_unlocked()?;
        let recoveries = broker
            .recoveries_unlocked()?
            .into_iter()
            .map(|(receipt, r)| RecoveryInfo {
                kind: r.change.kind,
                path: r.change.path,
                id: r.change.id,
                backup_path: receipt.with_extension("backup"),
                completed: r.completed,
            })
            .collect();
        Ok(ChangesView {
            reviews,
            recoveries,
        })
    }

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
                let mut missing = Vec::new();
                let mut current = path;
                let base = loop {
                    missing.push(
                        current
                            .file_name()
                            .ok_or_else(|| error("Missing filename"))?,
                    );
                    current = current
                        .parent()
                        .ok_or_else(|| error("Missing parent directory"))?;
                    match fs::canonicalize(current) {
                        Ok(base) => break base,
                        Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(e),
                    }
                };
                let mut resolved = base;
                for (index, name) in missing.iter().rev().enumerate() {
                    if index > 0 {
                        let staged = self.current(&resolved)?;
                        if !staged.is_some_and(|c| c.kind == ChangeKind::Directory && !c.deleted) {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                "Parent folder does not exist",
                            ));
                        }
                    }
                    resolved.push(name);
                }
                resolved
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
    fn overlay(&self, path: &Path, change: &Option<Change>) -> io::Result<Option<ChangeKind>> {
        if let Some(change) = change {
            return Ok((!change.deleted).then_some(change.kind));
        }
        check_links(path)?;
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => Ok(Some(ChangeKind::Directory)),
            Ok(_) => Ok(Some(ChangeKind::File)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn require_parent_folder(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| error("Missing parent directory"))?;
        let change = self.current(parent)?;
        if self.overlay(parent, &change)? != Some(ChangeKind::Directory) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Parent folder does not exist",
            ));
        }
        Ok(())
    }
    fn list(&self, path: &Path, change: &Option<Change>) -> io::Result<Vec<String>> {
        let mut entries = match fs::read_dir(path) {
            Ok(read) => read
                .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect::<io::Result<Vec<_>>>()?,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && change
                        .as_ref()
                        .is_some_and(|c| c.kind == ChangeKind::Directory && !c.deleted) =>
            {
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        for review in self.reviews_unlocked()? {
            if review.path.parent() == Some(path) {
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
        Ok(entries)
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
        let reply = |staged: bool| Response {
            schema_version: 1,
            staged,
            data: None,
            entries: None,
            stat: None,
        };
        match request.operation {
            Operation::ReadFile => {
                if change.as_ref().is_some_and(|c| c.deleted) {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "File is deleted in staged work",
                    ));
                }
                if change
                    .as_ref()
                    .is_some_and(|c| c.kind == ChangeKind::Directory)
                {
                    return Err(error("Path is a folder"));
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
                    staged: change.is_some(),
                    data: Some(data),
                    ..reply(false)
                })
            }
            Operation::WriteFile
            | Operation::DeleteFile
            | Operation::CreateDirectory
            | Operation::RemoveDirectory => {
                if self
                    .recoveries_unlocked()?
                    .iter()
                    .any(|(_, r)| r.change.path == path && r.completed.is_none())
                {
                    return Err(error(
                        "Resolve the previous copy-back recovery before editing this file",
                    ));
                }
                let wants = match request.operation {
                    Operation::WriteFile | Operation::DeleteFile => ChangeKind::File,
                    _ => ChangeKind::Directory,
                };
                let existing = self.overlay(&path, &change)?;
                if let (Operation::CreateDirectory, Some(c)) = (&request.operation, &change)
                    && c.deleted
                    && c.kind == ChangeKind::Directory
                {
                    fs::remove_file(self.change(&path))?;
                    return Ok(reply(false));
                }
                if let Some(c) = &change
                    && c.kind != wants
                {
                    return Err(error(
                        "A different kind of change is already staged for this path",
                    ));
                }
                if let Some(found) = existing
                    && found != wants
                {
                    return Err(error(match wants {
                        ChangeKind::File => "Path is a folder",
                        ChangeKind::Directory => "Path is a file",
                    }));
                }
                match request.operation {
                    Operation::CreateDirectory => {
                        if existing.is_some() {
                            return Ok(reply(change.is_some()));
                        }
                        self.require_parent_folder(&path)?;
                    }
                    Operation::WriteFile => self.require_parent_folder(&path)?,
                    Operation::RemoveDirectory => {
                        if existing.is_none() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                "Folder does not exist",
                            ));
                        }
                        if !self.list(&path, &change)?.is_empty() {
                            return Err(error(
                                "Folder is not empty; stage removal of its contents first",
                            ));
                        }
                        if change.as_ref().is_some_and(|c| c.baseline.is_none()) {
                            fs::remove_file(self.change(&path))?;
                            return Ok(reply(false));
                        }
                    }
                    _ => {}
                }
                let deleted = matches!(
                    request.operation,
                    Operation::DeleteFile | Operation::RemoveDirectory
                );
                let baseline = match &change {
                    Some(c) => c.baseline.clone(),
                    None => {
                        let observed = self.private.join(format!("{}.observed", Self::key(&path)));
                        match bytes(&observed) {
                            Ok(data) if wants == ChangeKind::File => {
                                let observed: Observed =
                                    serde_json::from_slice(&data).map_err(io::Error::other)?;
                                if observed.path != path {
                                    return Err(error("Invalid observed file identity"));
                                }
                                Some(observed.digest)
                            }
                            Ok(_) => fingerprint(&path)?,
                            Err(e) if e.kind() == io::ErrorKind::NotFound => fingerprint(&path)?,
                            Err(e) => return Err(e),
                        }
                    }
                };
                if matches!(request.operation, Operation::WriteFile) {
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
                        kind: wants,
                    },
                )?;
                Ok(reply(true))
            }
            Operation::ListDirectory => Ok(Response {
                entries: Some(self.list(&path, &change)?),
                ..reply(false)
            }),
            Operation::Stat => {
                let mut stat = Stat {
                    exists: false,
                    kind: None,
                    size: None,
                    modified: None,
                };
                if let Some(kind) = self.overlay(&path, &change)? {
                    let source = match (kind, &change) {
                        (ChangeKind::File, Some(_)) => self.staged(&path),
                        _ => path.clone(),
                    };
                    stat.exists = true;
                    stat.kind = Some(kind);
                    if let Ok(meta) = fs::metadata(source) {
                        if kind == ChangeKind::File {
                            stat.size = Some(meta.len());
                        }
                        stat.modified = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_secs());
                    }
                }
                Ok(Response {
                    stat: Some(stat),
                    ..reply(false)
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
            let staged_digest = match (change.kind, change.deleted) {
                (_, true) => None,
                (ChangeKind::Directory, false) => Some(DIRECTORY.to_owned()),
                (ChangeKind::File, false) => fingerprint(&staged_path)?,
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
                kind: change.kind,
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

    pub fn apply_review(&self, review: &Review) -> io::Result<()> {
        let _lock = self.lock()?;
        let path = self.authorize(&review.path)?;
        let change = self
            .current(&path)?
            .ok_or_else(|| error("Staged change no longer exists"))?;
        if change.baseline != review.baseline
            || change.deleted != review.deleted
            || change.kind != review.kind
            || fingerprint(&path)? != review.baseline
        {
            return Err(error(
                "Original changed; resolve the conflict before copy-back",
            ));
        }
        let staged = match (change.kind, change.deleted) {
            (ChangeKind::File, false) => Some(bytes(&self.staged(&path))?),
            _ => None,
        };
        let digest = match change.kind {
            ChangeKind::File => staged.as_ref().map(|b| hash(b)),
            ChangeKind::Directory => (!change.deleted).then(|| DIRECTORY.to_owned()),
        };
        if digest != review.staged_digest {
            return Err(error("Staged content changed; review it again"));
        }
        if !change.deleted && !path.parent().is_some_and(|p| p.is_dir()) {
            return Err(error("Apply the parent folder first"));
        }
        if change.deleted && change.kind == ChangeKind::Directory {
            let mut contents = fs::read_dir(&path)?;
            if contents.next().is_some() {
                return Err(error("Folder is not empty; apply its removals first"));
            }
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
        if change.kind == ChangeKind::File && change.baseline.is_some() {
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
        match (change.kind, staged) {
            (ChangeKind::Directory, _) if change.deleted => fs::remove_dir(&path)?,
            (ChangeKind::Directory, _) => fs::create_dir(&path)?,
            (ChangeKind::File, Some(data)) => atomic(&path, &data)?,
            (ChangeKind::File, None) => {
                if path.exists() {
                    fs::remove_file(&path)?;
                }
            }
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
        match recovery.change.kind {
            ChangeKind::Directory => {
                if recovery.change.baseline.is_some() {
                    if !path.is_dir() {
                        fs::create_dir(&path)?;
                    }
                } else if path.is_dir() {
                    fs::remove_dir(&path)?;
                }
            }
            ChangeKind::File => {
                if recovery.change.baseline.is_some() {
                    let backup = bytes(&receipt.with_extension("backup"))?;
                    if Some(hash(&backup)) != recovery.change.baseline {
                        return Err(error("Recovery backup integrity check failed"));
                    }
                    atomic(&path, &backup)?;
                } else if path.exists() {
                    fs::remove_file(&path)?;
                }
            }
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
                kind: r.change.kind,
                path: r.change.path,
                id: r.change.id,
                backup_path: receipt.with_extension("backup"),
                completed: r.completed,
            })
            .collect())
    }

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
    fn inspection_lists_changes_and_recoveries_without_creating_or_pruning() {
        let f = Fixture::new(false);
        let missing = f.root.join("not-created");
        let empty = Broker::inspect(missing.clone(), "fixture").unwrap();
        assert!(empty.reviews.is_empty() && empty.recoveries.is_empty());
        assert!(!missing.exists());
        f.write(b"changed");
        let lock = f.broker.lock().unwrap();
        assert!(Broker::inspect(f.broker.root.clone(), "fixture").is_err());
        drop(lock);
        let changes = Broker::inspect(f.broker.root.clone(), "fixture").unwrap();
        assert_eq!(changes.reviews.len(), 1);
        assert!(changes.recoveries.is_empty());
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
        assert_eq!(
            fs::read(&changes.reviews[0].staged_path).unwrap(),
            b"changed"
        );
        f.broker.apply_review(&changes.reviews[0]).unwrap();
        let after = Broker::inspect(f.broker.root.clone(), "fixture").unwrap();
        assert!(after.reviews.is_empty());
        assert_eq!(after.recoveries.len(), 1);
        assert!(after.recoveries[0].backup_path.exists());
        assert_eq!(fs::read(&f.file).unwrap(), b"changed");
    }

    #[test]
    fn approved_actions_reject_stale_reviews_and_newer_external_edits() {
        let f = Fixture::new(false);
        f.write(b"first");
        let stale = Broker::inspect(f.broker.root.clone(), "fixture")
            .unwrap()
            .reviews[0]
            .approval_token()
            .unwrap();
        f.write(b"second");
        assert!(Broker::apply_approved(f.broker.root.clone(), "fixture", "apply", &stale).is_err());
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
        let current = Broker::inspect(f.broker.root.clone(), "fixture")
            .unwrap()
            .reviews[0]
            .approval_token()
            .unwrap();
        Broker::apply_approved(f.broker.root.clone(), "fixture", "apply", &current).unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"second");
        let recovery = Broker::inspect(f.broker.root.clone(), "fixture")
            .unwrap()
            .recoveries[0]
            .approval_token()
            .unwrap();
        fs::write(&f.file, b"external edit").unwrap();
        assert!(
            Broker::apply_approved(f.broker.root.clone(), "fixture", "restore", &recovery).is_err()
        );
        assert_eq!(fs::read(&f.file).unwrap(), b"external edit");
        fs::write(&f.file, b"second").unwrap();
        Broker::apply_approved(f.broker.root.clone(), "fixture", "restore", &recovery).unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
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

    #[test]
    fn folders_stat_and_moves_are_staged_reviewable_and_undoable() {
        let f = Fixture::new(true);
        let root = f.file.parent().unwrap().to_owned();
        let sub = root.join("sub");
        let nested = sub.join("nested");
        let moved = nested.join("moved.txt");
        let stat = f
            .request(Operation::Stat, &f.file, &[])
            .unwrap()
            .stat
            .unwrap();
        assert!(stat.exists && stat.kind == Some(ChangeKind::File) && stat.size == Some(8));
        assert!(f.request(Operation::CreateDirectory, &nested, &[]).is_err());
        assert!(
            f.request(Operation::CreateDirectory, &sub, &[])
                .unwrap()
                .staged
        );
        f.request(Operation::CreateDirectory, &nested, &[]).unwrap();
        f.request(Operation::CreateDirectory, &sub, &[]).unwrap();
        f.request(Operation::CreateDirectory, &root, &[]).unwrap();
        assert!(!sub.exists());
        let stat = f
            .request(Operation::Stat, &nested, &[])
            .unwrap()
            .stat
            .unwrap();
        assert!(stat.exists && stat.kind == Some(ChangeKind::Directory));
        let data = f
            .request(Operation::ReadFile, &f.file, &[])
            .unwrap()
            .data
            .unwrap();
        f.request(Operation::WriteFile, &moved, &data).unwrap();
        f.request(Operation::DeleteFile, &f.file, &[]).unwrap();
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
        assert!(!sub.exists());
        let names = |dir: &Path| {
            f.request(Operation::ListDirectory, dir, &[])
                .unwrap()
                .entries
                .unwrap()
        };
        assert_eq!(names(&root), vec!["sub"]);
        assert_eq!(names(&sub), vec!["nested"]);
        assert_eq!(names(&nested), vec!["moved.txt"]);
        assert_eq!(
            f.request(Operation::ReadFile, &moved, &[])
                .unwrap()
                .data
                .unwrap(),
            b"original"
        );
        assert!(
            !f.request(Operation::Stat, &f.file, &[])
                .unwrap()
                .stat
                .unwrap()
                .exists
        );
        assert!(f.request(Operation::RemoveDirectory, &sub, &[]).is_err());
        assert!(f.request(Operation::WriteFile, &sub, b"x").is_err());
        assert!(f.request(Operation::ReadFile, &sub, &[]).is_err());
        let reviews = f.broker.reviews().unwrap();
        assert_eq!(reviews.len(), 4);
        let child = reviews
            .iter()
            .find(|r| r.path.ends_with("moved.txt"))
            .unwrap();
        assert!(f.broker.apply_review(child).is_err());
        assert!(f.broker.recoveries().unwrap().is_empty());
        for review in f.broker.reviews().unwrap() {
            f.broker.apply_review(&review).unwrap();
        }
        assert_eq!(fs::read(&moved).unwrap(), b"original");
        assert!(!f.file.exists());
        assert!(f.broker.reviews().unwrap().is_empty());
        let mut recoveries = f.broker.recoveries().unwrap();
        assert_eq!(recoveries.len(), 4);
        recoveries.sort_by(|a, b| {
            b.path
                .components()
                .count()
                .cmp(&a.path.components().count())
        });
        for info in &recoveries {
            f.broker.restore(info).unwrap();
        }
        assert_eq!(fs::read(&f.file).unwrap(), b"original");
        assert!(!sub.exists());
    }

    #[test]
    fn folder_removal_is_reviewable_cancelable_and_restorable() {
        let f = Fixture::new(true);
        let root = f.file.parent().unwrap().to_owned();
        let empty = root.join("empty");
        fs::create_dir(&empty).unwrap();
        f.request(Operation::RemoveDirectory, &empty, &[]).unwrap();
        assert!(empty.exists());
        assert!(
            !f.request(Operation::Stat, &empty, &[])
                .unwrap()
                .stat
                .unwrap()
                .exists
        );
        let review = f.broker.reviews().unwrap().remove(0);
        assert!(review.deleted && review.kind == ChangeKind::Directory);
        f.request(Operation::CreateDirectory, &empty, &[]).unwrap();
        assert!(f.broker.reviews().unwrap().is_empty());
        f.request(Operation::RemoveDirectory, &empty, &[]).unwrap();
        f.broker
            .apply_review(&f.broker.reviews().unwrap().remove(0))
            .unwrap();
        assert!(!empty.exists());
        f.broker
            .restore(&f.broker.recoveries().unwrap().remove(0))
            .unwrap();
        assert!(empty.is_dir());
        let scratch = root.join("scratch");
        f.request(Operation::CreateDirectory, &scratch, &[])
            .unwrap();
        f.request(Operation::RemoveDirectory, &scratch, &[])
            .unwrap();
        assert!(f.broker.reviews().unwrap().is_empty());
        fs::write(empty.join("file.txt"), b"x").unwrap();
        assert!(f.request(Operation::RemoveDirectory, &empty, &[]).is_err());
    }
}
