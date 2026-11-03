#![allow(clippy::unwrap_used)]
use knot_core::*;
use knot_store::FsBackend;
use std::{
    fs,
    sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    },
};
struct Gate {
    inner: FsBackend,
    first: AtomicBool,
    entered: Barrier,
    resume: Barrier,
}
impl Backend for Gate {
    fn is_bare(&self) -> bool {
        self.inner.is_bare()
    }
    fn author(&self) -> Result<Option<String>> {
        self.inner.author()
    }
    fn read_meta(&self, n: &str) -> Result<Option<String>> {
        self.inner.read_meta(n)
    }
    fn write_meta(&self, n: &str, c: Option<&str>) -> Result<()> {
        self.inner.write_meta(n, c)
    }
    fn list_meta(&self, p: &str) -> Result<Vec<String>> {
        self.inner.list_meta(p)
    }
    fn put_object(&self, k: ObjectKind, p: &[u8]) -> Result<ObjectId> {
        self.inner.put_object(k, p)
    }
    fn put_versioned_object(&self, k: ObjectKind, v: u32, p: &[u8]) -> Result<ObjectId> {
        self.inner.put_versioned_object(k, v, p)
    }
    fn object_version(&self, id: &ObjectId) -> Result<u32> {
        self.inner.object_version(id)
    }
    fn get_object(&self, id: &ObjectId) -> Result<(ObjectKind, Vec<u8>)> {
        self.inner.get_object(id)
    }
    fn object_kind(&self, id: &ObjectId) -> Result<ObjectKind> {
        self.inner.object_kind(id)
    }
    fn verify_blob(&self, id: &ObjectId) -> Result<u64> {
        self.inner.verify_blob(id)
    }
    fn list_objects(&self) -> Result<Vec<ObjectId>> {
        self.inner.list_objects()
    }
    fn list_files(&self, i: bool) -> Result<Vec<String>> {
        self.inner.list_files(i)
    }
    fn file_state(&self, p: &str, h: bool) -> Result<Option<FileState>> {
        if self.first.swap(false, Ordering::SeqCst) {
            self.entered.wait();
            self.resume.wait();
        }
        self.inner.file_state(p, h)
    }
    fn store_file(&self, p: &str, h: bool) -> Result<Option<FileState>> {
        self.inner.store_file(p, h)
    }
    fn write_file(&self, p: &str, s: Option<&FileState>) -> Result<()> {
        self.inner.write_file(p, s)
    }
}
#[test]
fn overlapping_status_calls_recheck_objects_independently() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    repo.write_asset("part", b"A").unwrap();
    repo.commit("initial").unwrap();
    let snapshot = repo.status().unwrap().staged_snapshot;
    drop(repo);
    let repo = Repository::open(Gate {
        inner: FsBackend::open(root).unwrap(),
        first: AtomicBool::new(true),
        entered: Barrier::new(2),
        resume: Barrier::new(2),
    })
    .unwrap();
    let path = root.join(format!(
        ".knot/objects/sha256/{}/{}",
        &snapshot.as_str()[..2],
        &snapshot.as_str()[2..]
    ));
    std::thread::scope(|scope| {
        let first = scope.spawn(|| repo.status());
        repo.backend().entered.wait();
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"corrupt snapshot").unwrap();
        let independent = repo.status();
        repo.backend().resume.wait();
        assert!(first.join().unwrap().is_ok());
        assert!(
            independent.is_err(),
            "independent calls must recheck disk integrity"
        );
        assert!(repo.status().is_err());
        fs::write(&path, original).unwrap();
    });
    drop(repo);
}
