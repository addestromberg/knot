use knot_core::{Backend, Error, ObjectKind, Result, Snapshot, canonical, validate_ref_name};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub format_version: u32,
    pub hash: String,
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bare: bool,
}
fn is_false(value: &bool) -> bool {
    !value
}

pub struct FsBackend {
    pub(crate) root: PathBuf,
    pub(crate) meta: PathBuf,
    pub(crate) config: Config,
    _lock: File,
}

impl FsBackend {
    pub fn init_bare(path: impl AsRef<Path>, author: Option<String>) -> Result<Self> {
        let mut backend = Self::init(path, author)?;
        backend.config.bare = true;
        backend.config.format_version = 3;
        backend.write_metadata(
            "config.toml",
            Some(&toml::to_string(&backend.config).map_err(|e| Error::Integrity(e.to_string()))?),
        )?;
        Ok(backend)
    }
    pub fn init(path: impl AsRef<Path>, author: Option<String>) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;
        let root = fs::canonicalize(path)?;
        let destination = root.join(".knot");
        ensure_absent(&destination)?;
        let staging = tempfile::Builder::new()
            .prefix(".knot-init-")
            .tempdir_in(&root)?;
        let meta = staging.path().to_owned();
        for sub in [
            "objects/sha256",
            "refs/heads",
            "refs/tags",
            "refs/issues",
            "logs",
            "locks",
            "cache",
        ] {
            fs::create_dir_all(meta.join(sub))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(meta.join("locks/repository"))?;
        lock.try_lock().map_err(lock_error)?;
        let mut backend = Self {
            root,
            meta,
            config: Config {
                format_version: 1,
                hash: "sha256".into(),
                author,
                bare: false,
            },
            _lock: lock,
        };
        let empty = canonical(&Snapshot::default())?;
        let id = backend.put_object(ObjectKind::Snapshot, &empty)?;
        backend.write_meta("index", Some(&format!("{id}\n")))?;
        backend.write_meta("HEAD", Some("ref: refs/heads/main\n"))?;
        let config = toml::to_string(&backend.config).map_err(|e| Error::Invalid(e.to_string()))?;
        backend.write_meta("config.toml", Some(&config))?;
        backend._lock.sync_all()?;
        for sub in [
            "objects/sha256",
            "objects",
            "refs/heads",
            "refs/tags",
            "refs/issues",
            "refs",
            "logs",
            "locks",
            "cache",
        ] {
            sync_directory(&backend.meta.join(sub))?;
        }
        sync_directory(&backend.meta)?;
        ensure_absent(&destination)?;
        fs::rename(&backend.meta, &destination).map_err(|e| {
            if matches!(
                e.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
            ) {
                Error::Conflict(".knot already exists; initialization never overwrites it".into())
            } else {
                Error::Io(e)
            }
        })?;
        backend.meta = destination;
        sync_directory(&backend.root)?;
        Ok(backend)
    }

    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let mut path = fs::canonicalize(start)?;
        if path.is_file() {
            path.pop();
        }
        loop {
            match fs::symlink_metadata(path.join(".knot")) {
                Ok(_) => return Self::open(path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            if !path.pop() {
                return Err(Error::NotFound("no .knot repository found".into()));
            }
        }
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let root = fs::canonicalize(path)?;
        let meta = root.join(".knot");
        let stat = fs::symlink_metadata(&meta)?;
        if !stat.is_dir() || stat.file_type().is_symlink() {
            return Err(Error::Integrity(".knot must be a real directory".into()));
        }
        let lock_path = checked_join(&meta, "locks/repository")?;
        let lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
        lock.try_lock().map_err(lock_error)?;
        let config_path = checked_join(&meta, "config.toml")?;
        let config: Config = toml::from_str(&fs::read_to_string(config_path)?)
            .map_err(|e| Error::Integrity(format!("invalid config.toml: {e}")))?;
        if ![1, 2, 3].contains(&config.format_version) || config.hash != "sha256" {
            return Err(Error::Integrity(
                "unsupported repository format or hash algorithm".into(),
            ));
        }
        Ok(Self {
            root,
            meta,
            config,
            _lock: lock,
        })
    }
    pub(crate) fn ensure_m2_format(&self) -> Result<()> {
        self.ensure_format(2)
    }
    pub(crate) fn ensure_format(&self, minimum: u32) -> Result<()> {
        let text = self
            .read_metadata("config.toml")?
            .ok_or_else(|| Error::Integrity("missing config.toml".into()))?;
        let mut config: Config =
            toml::from_str(&text).map_err(|e| Error::Integrity(e.to_string()))?;
        if ![1, 2, 3].contains(&config.format_version) {
            return Err(Error::Integrity("unsupported repository format".into()));
        }
        if config.format_version < minimum {
            config.format_version = minimum;
            self.write_metadata(
                "config.toml",
                Some(&toml::to_string(&config).map_err(|e| Error::Integrity(e.to_string()))?),
            )?;
        }
        Ok(())
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn meta_path(&self, name: &str) -> Result<PathBuf> {
        validate_meta_name(name)?;
        checked_join(&self.meta, name)
    }
    pub(crate) fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("missing parent directory".into()))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|e| Error::Io(e.error))?;
        sync_directory_chain(parent, &self.meta)?;
        Ok(())
    }
    pub(crate) fn read_metadata(&self, name: &str) -> Result<Option<String>> {
        let path = self.meta_path(name)?;
        match fs::read_to_string(path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub(crate) fn write_metadata(&self, name: &str, content: Option<&str>) -> Result<()> {
        let path = self.meta_path(name)?;
        if let Some(content) = content {
            if name.starts_with("logs/") && path.exists() {
                if fs::read_to_string(&path)? != content {
                    return Err(Error::Integrity(
                        "attempted to replace transaction log".into(),
                    ));
                }
                return Ok(());
            }
            self.atomic_write(&path, content.as_bytes())
        } else {
            match fs::remove_file(&path) {
                Ok(()) => {
                    if let Some(parent) = path.parent() {
                        sync_directory(parent)?;
                    }
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
    pub(crate) fn list_metadata(&self, prefix: &str) -> Result<Vec<String>> {
        let prefix = prefix.trim_end_matches('/');
        if ![
            "refs",
            "refs/heads",
            "refs/tags",
            "refs/issues",
            "refs/reviews",
            "refs/releases",
            "refs/remotes",
            "remotes",
            "logs",
        ]
        .contains(&prefix)
        {
            return Err(Error::Invalid("unsupported metadata listing".into()));
        }
        let mut out = Vec::new();
        let dir = checked_join(&self.meta, prefix)?;
        self.walk_metadata(&dir, &mut out)?;
        out.sort();
        Ok(out)
    }
    fn walk_metadata(&self, dir: &Path, out: &mut Vec<String>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let stat = entry.file_type()?;
            if stat.is_symlink() {
                return Err(Error::Integrity("symlink in repository metadata".into()));
            }
            if stat.is_dir() {
                self.walk_metadata(&entry.path(), out)?;
            } else if stat.is_file() {
                if entry.file_name().to_string_lossy().starts_with(".tmp") {
                    continue;
                }
                let path = entry.path();
                let relative = path
                    .strip_prefix(&self.meta)
                    .map_err(|e| Error::Integrity(e.to_string()))?;
                let name = relative
                    .to_str()
                    .ok_or_else(|| Error::Integrity("non-UTF-8 metadata path".into()))?
                    .replace('\\', "/");
                validate_meta_name(&name)?;
                out.push(name);
            } else {
                return Err(Error::Integrity(
                    "special file in repository metadata".into(),
                ));
            }
        }
        Ok(())
    }
}

fn lock_error(error: fs::TryLockError) -> Error {
    match error {
        fs::TryLockError::WouldBlock => Error::Busy,
        fs::TryLockError::Error(e) => Error::Io(e),
    }
}

pub(crate) fn checked_join(base: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(Error::Invalid(format!("unsafe relative path: {relative}")));
    }
    let relative_path = Path::new(relative);
    if relative_path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::Invalid("path escapes repository".into()));
    }
    let mut path = base.to_path_buf();
    for component in relative_path.components() {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::Integrity(format!(
                    "symbolic links are unsupported: {}",
                    path.display()
                )));
            }
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}

fn validate_meta_name(name: &str) -> Result<()> {
    if ["HEAD", "index", "config.toml", "transaction.json"].contains(&name) {
        return Ok(());
    }
    for prefix in [
        "refs/heads/",
        "refs/tags/",
        "refs/releases/",
        "refs/remotes/",
        "remotes/",
    ] {
        if let Some(tail) = name.strip_prefix(prefix) {
            return validate_ref_name(tail);
        }
    }
    if let Some(tail) = name.strip_prefix("refs/reviews/") {
        tail.parse::<knot_core::ObjectId>()?;
        return Ok(());
    }
    if let Some(tail) = name.strip_prefix("refs/issues/") {
        let id = uuid::Uuid::parse_str(tail).map_err(|e| Error::Integrity(e.to_string()))?;
        if id.to_string() == tail {
            return Ok(());
        }
    }
    if let Some(tail) = name
        .strip_prefix("logs/")
        .and_then(|s| s.strip_suffix(".json"))
    {
        let id = uuid::Uuid::parse_str(tail).map_err(|e| Error::Integrity(e.to_string()))?;
        if id.to_string() == tail {
            return Ok(());
        }
    }
    Err(Error::Invalid(format!("invalid metadata path: {name}")))
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl Backend for FsBackend {
    fn is_bare(&self) -> bool {
        self.config.bare
    }
    fn author(&self) -> Result<Option<String>> {
        Ok(self.config.author.clone())
    }
    fn read_meta(&self, name: &str) -> Result<Option<String>> {
        self.read_metadata(name)
    }
    fn write_meta(&self, name: &str, content: Option<&str>) -> Result<()> {
        self.write_metadata(name, content)
    }
    fn list_meta(&self, prefix: &str) -> Result<Vec<String>> {
        self.list_metadata(prefix)
    }
    fn put_object(&self, kind: ObjectKind, payload: &[u8]) -> Result<knot_core::ObjectId> {
        self.write_object(kind, payload)
    }
    fn put_versioned_object(
        &self,
        kind: ObjectKind,
        version: u32,
        payload: &[u8],
    ) -> Result<knot_core::ObjectId> {
        self.write_versioned_object(kind, version, payload)
    }
    fn object_version(&self, id: &knot_core::ObjectId) -> Result<u32> {
        Ok(self.open_object(id)?.2)
    }
    fn get_object(&self, id: &knot_core::ObjectId) -> Result<(ObjectKind, Vec<u8>)> {
        self.read_object(id)
    }
    fn object_kind(&self, id: &knot_core::ObjectId) -> Result<ObjectKind> {
        Ok(self.open_object(id)?.1)
    }
    fn verify_blob(&self, id: &knot_core::ObjectId) -> Result<u64> {
        self.check_blob(id)
    }
    fn list_objects(&self) -> Result<Vec<knot_core::ObjectId>> {
        self.object_ids()
    }
    fn list_files(&self, include_ignored: bool) -> Result<Vec<String>> {
        self.working_files(include_ignored)
    }
    fn file_state(&self, path: &str, hint: bool) -> Result<Option<knot_core::FileState>> {
        self.working_state(path, hint, false)
    }
    fn store_file(&self, path: &str, hint: bool) -> Result<Option<knot_core::FileState>> {
        self.working_state(path, hint, true)
    }
    fn write_file(&self, path: &str, state: Option<&knot_core::FileState>) -> Result<()> {
        self.restore_file(path, state)
    }
}

fn ensure_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => Err(Error::Conflict(
            ".knot already exists; initialization never overwrites it".into(),
        )),
    }
}

pub(crate) fn sync_directory_chain(path: &Path, stop: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(dir) = current {
        sync_directory(dir)?;
        if dir == stop {
            break;
        }
        current = dir.parent();
    }
    Ok(())
}
