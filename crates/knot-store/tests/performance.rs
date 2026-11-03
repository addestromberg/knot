#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use knot_store::FsBackend;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
};
use tempfile::tempdir;

struct CountingBackend {
    inner: FsBackend,
    snapshots_read: Cell<usize>,
    snapshots_written: Cell<usize>,
    transactions: Cell<usize>,
    hashed: RefCell<BTreeMap<String, usize>>,
    structured_reads: RefCell<BTreeMap<ObjectId, usize>>,
    blob_checks: RefCell<BTreeMap<ObjectId, usize>>,
}
impl Backend for CountingBackend {
    fn is_bare(&self) -> bool {
        self.inner.is_bare()
    }
    fn author(&self) -> Result<Option<String>> {
        self.inner.author()
    }
    fn read_meta(&self, name: &str) -> Result<Option<String>> {
        self.inner.read_meta(name)
    }
    fn write_meta(&self, name: &str, content: Option<&str>) -> Result<()> {
        if name == "transaction.json" && content.is_some() {
            self.transactions.set(self.transactions.get() + 1);
        }
        self.inner.write_meta(name, content)?;
        Ok(())
    }
    fn list_meta(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list_meta(prefix)
    }
    fn put_object(&self, kind: ObjectKind, payload: &[u8]) -> Result<ObjectId> {
        self.inner.put_object(kind, payload)
    }
    fn put_versioned_object(
        &self,
        kind: ObjectKind,
        version: u32,
        payload: &[u8],
    ) -> Result<ObjectId> {
        if kind == ObjectKind::Snapshot {
            self.snapshots_written.set(self.snapshots_written.get() + 1);
        }
        self.inner.put_versioned_object(kind, version, payload)
    }
    fn object_version(&self, id: &ObjectId) -> Result<u32> {
        self.inner.object_version(id)
    }
    fn get_object(&self, id: &ObjectId) -> Result<(ObjectKind, Vec<u8>)> {
        let result = self.inner.get_object(id)?;
        if result.0 != ObjectKind::Blob {
            *self
                .structured_reads
                .borrow_mut()
                .entry(id.clone())
                .or_default() += 1;
        }
        if result.0 == ObjectKind::Snapshot {
            self.snapshots_read.set(self.snapshots_read.get() + 1);
        }
        Ok(result)
    }
    fn object_kind(&self, id: &ObjectId) -> Result<ObjectKind> {
        self.inner.object_kind(id)
    }
    fn verify_blob(&self, id: &ObjectId) -> Result<u64> {
        *self.blob_checks.borrow_mut().entry(id.clone()).or_default() += 1;
        self.inner.verify_blob(id)
    }
    fn list_objects(&self) -> Result<Vec<ObjectId>> {
        self.inner.list_objects()
    }
    fn list_files(&self, ignored: bool) -> Result<Vec<String>> {
        self.inner.list_files(ignored)
    }
    fn file_state(&self, path: &str, hint: bool) -> Result<Option<FileState>> {
        self.hashed
            .borrow_mut()
            .entry(path.to_owned())
            .and_modify(|n| *n += 1)
            .or_insert(1);
        self.inner.file_state(path, hint)
    }
    fn store_file(&self, path: &str, hint: bool) -> Result<Option<FileState>> {
        self.inner.store_file(path, hint)
    }
    fn write_file(&self, path: &str, state: Option<&FileState>) -> Result<()> {
        self.inner.write_file(path, state)?;
        Ok(())
    }
}
#[test]
fn bulk_overrides_hash_each_input_once_and_publish_one_transaction() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Repository<FsBackend>>();
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    fs::write(dir.path().join("shared"), "shared geometry").unwrap();
    for n in 0..24 {
        fs::write(dir.path().join(format!("part-{n}")), "part").unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    for n in 0..24 {
        repo.add_dependency(
            &format!("part-{n}"),
            "shared",
            DependencyRequest::Revision(None),
        )
        .unwrap();
    }
    repo.commit("Initial").unwrap();
    fs::write(dir.path().join("shared"), "changed geometry").unwrap();
    repo.add(&["shared".into()]).unwrap();
    let subjects: Vec<_> = repo
        .verification(None)
        .unwrap()
        .dependencies
        .into_iter()
        .map(|e| e.subject)
        .collect();
    drop(repo);
    let repo = Repository::open(CountingBackend {
        inner: FsBackend::open(dir.path()).unwrap(),
        snapshots_read: Cell::new(0),
        snapshots_written: Cell::new(0),
        transactions: Cell::new(0),
        hashed: RefCell::default(),
        structured_reads: RefCell::default(),
        blob_checks: RefCell::default(),
    })
    .unwrap();
    let records = repo
        .override_dependency_reviews(&subjects, "Operator", "Explicit override")
        .unwrap();
    assert_eq!(records.len(), 24);
    assert_eq!(
        repo.backend().snapshots_written.get(),
        1,
        "only the final snapshot is stored"
    );
    assert_eq!(
        repo.backend().transactions.get(),
        1,
        "all reviews and staging share a journal"
    );
    assert_eq!(repo.backend().hashed.borrow().len(), 25);
    assert!(
        repo.backend()
            .hashed
            .borrow()
            .values()
            .all(|count| *count == 1),
        "shared engineering inputs must not be rehashed per edge"
    );
    repo.backend().snapshots_read.set(0);
    repo.backend().structured_reads.borrow_mut().clear();
    let status = repo.status().unwrap();
    assert!(
        status
            .verification
            .dependencies
            .iter()
            .all(|e| e.overridden && e.state == VerificationState::Verified)
    );
    assert!(
        repo.backend().snapshots_read.get() <= 3,
        "status must reuse the checked snapshots across review records"
    );
    assert!(
        repo.backend()
            .structured_reads
            .borrow()
            .values()
            .all(|count| *count == 1),
        "immutable payloads must be read once per operation"
    );
    let snapshot = status.staged_snapshot;
    for id in records {
        assert_eq!(
            repo.load::<ValidationRecord>(&id)
                .unwrap()
                .override_snapshot,
            Some(snapshot.clone())
        );
    }
    // A new call must recheck both working bytes and immutable objects.
    fs::write(dir.path().join("shared"), "changed geometrY").unwrap();
    assert_eq!(repo.status().unwrap().unstaged.len(), 1);
    let object = dir.path().join(format!(
        ".knot/objects/sha256/{}/{}",
        &snapshot.as_str()[..2],
        &snapshot.as_str()[2..]
    ));
    let original = fs::read(&object).unwrap();
    fs::write(&object, b"corrupt").unwrap();
    assert!(matches!(repo.status(), Err(Error::Integrity(_))));
    fs::write(object, original).unwrap();
    assert_eq!(repo.status().unwrap().unstaged.len(), 1);
}

#[test]
fn fsck_checks_shared_content_once_and_discards_trust_afterward() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    fs::write(dir.path().join("part"), vec![b'x'; 4096]).unwrap();
    fs::write(dir.path().join("contract"), "initial").unwrap();
    repo.add(&[".".into()]).unwrap();
    repo.commit("Initial").unwrap();
    for n in 0..10 {
        fs::write(dir.path().join("contract"), format!("fit-{n}")).unwrap();
        repo.set_interface("part", "fit", "contract").unwrap();
        repo.commit(&format!("Update fit {n}")).unwrap();
    }
    let blob = repo
        .graph(None)
        .unwrap()
        .nodes
        .into_iter()
        .find(|n| n.path == "part")
        .unwrap()
        .blob;
    drop(repo);
    let repo = Repository::open(CountingBackend {
        inner: FsBackend::open(dir.path()).unwrap(),
        snapshots_read: Cell::new(0),
        snapshots_written: Cell::new(0),
        transactions: Cell::new(0),
        hashed: RefCell::default(),
        structured_reads: RefCell::default(),
        blob_checks: RefCell::default(),
    })
    .unwrap();
    repo.fsck().unwrap();
    assert_eq!(repo.backend().blob_checks.borrow()[&blob], 1);
    assert!(
        repo.backend()
            .blob_checks
            .borrow()
            .values()
            .all(|n| *n == 1)
    );
    assert!(
        repo.backend()
            .structured_reads
            .borrow()
            .values()
            .all(|n| *n == 1)
    );
    let path = dir.path().join(format!(
        ".knot/objects/sha256/{}/{}",
        &blob.as_str()[..2],
        &blob.as_str()[2..]
    ));
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"corrupted between calls").unwrap();
    assert!(repo.fsck().is_err());
    fs::write(path, original).unwrap();
    repo.fsck().unwrap();
}
