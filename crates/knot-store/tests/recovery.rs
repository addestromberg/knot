#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use knot_store::FsBackend;
use std::{cell::Cell, fs};
use tempfile::tempdir;

struct FaultBackend {
    inner: FsBackend,
    remaining: Cell<Option<usize>>,
}
impl FaultBackend {
    fn after_write(&self) -> Result<()> {
        if let Some(remaining) = self.remaining.get() {
            if remaining == 1 {
                self.remaining.set(None);
                return Err(Error::Io(std::io::Error::other(
                    "injected interruption after durable write",
                )));
            }
            self.remaining.set(Some(remaining - 1));
        }
        Ok(())
    }
}
impl Backend for FaultBackend {
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
        self.inner.write_meta(name, content)?;
        self.after_write()
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
        self.inner.put_versioned_object(kind, version, payload)
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
    fn list_files(&self, ignored: bool) -> Result<Vec<String>> {
        self.inner.list_files(ignored)
    }
    fn file_state(&self, path: &str, hint: bool) -> Result<Option<FileState>> {
        self.inner.file_state(path, hint)
    }
    fn store_file(&self, path: &str, hint: bool) -> Result<Option<FileState>> {
        self.inner.store_file(path, hint)
    }
    fn write_file(&self, path: &str, state: Option<&FileState>) -> Result<()> {
        self.inner.write_file(path, state)?;
        self.after_write()
    }
}
fn seed(root: &std::path::Path) -> (ObjectId, ObjectId) {
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    fs::write(root.join("a"), "one").unwrap();
    fs::write(root.join("old"), "old").unwrap();
    repo.add(&[".".into()]).unwrap();
    let first = repo.commit("First").unwrap();
    repo.create_ref("v1", "HEAD", true).unwrap();
    fs::write(root.join("a"), "two").unwrap();
    fs::remove_file(root.join("old")).unwrap();
    fs::write(root.join("new"), "new").unwrap();
    repo.add(&[".".into()]).unwrap();
    let second = repo.commit("Second").unwrap();
    (first, second)
}

#[test]
fn synchronized_ref_publication_recovers_after_each_durable_write() {
    for boundary in 1..=4 {
        let dir = tempdir().unwrap();
        let (_, head) = seed(dir.path());
        let backend = FaultBackend {
            inner: FsBackend::open(dir.path()).unwrap(),
            remaining: Cell::new(Some(boundary)),
        };
        let repo = Repository::open(backend).unwrap();
        let _ = repo.publish_references(
            &[ReferenceUpdate {
                name: "refs/remotes/peer/main".into(),
                before: None,
                after: head.clone(),
            }],
            None,
            false,
        );
        drop(repo);
        let repo = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
        if repo.status().unwrap().recovery_required {
            repo.recover(false).unwrap();
        }
        assert_eq!(
            repo.backend().read_meta("refs/remotes/peer/main").unwrap(),
            Some(format!("{head}\n"))
        );
        assert_eq!(repo.head_id().unwrap(), Some(head));
        repo.fsck().unwrap();
    }
}

#[test]
fn checkout_recovers_after_every_durable_write_boundary() {
    for boundary in 1..=9 {
        let dir = tempdir().unwrap();
        let (first, _) = seed(dir.path());
        let backend = FaultBackend {
            inner: FsBackend::open(dir.path()).unwrap(),
            remaining: Cell::new(Some(boundary)),
        };
        let repo = Repository::open(backend).unwrap();
        let _ = repo.apply_checkout(repo.plan_checkout("v1").unwrap());
        drop(repo);
        let recovered = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
        if recovered
            .backend()
            .read_meta("transaction.json")
            .unwrap()
            .is_some()
        {
            recovered.recover(false).unwrap();
        }
        assert_eq!(
            recovered.head_id().unwrap(),
            Some(first),
            "boundary {boundary}"
        );
        assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "one");
        assert_eq!(fs::read_to_string(dir.path().join("old")).unwrap(), "old");
        assert!(!dir.path().join("new").exists());
        assert!(!recovered.status().unwrap().recovery_required);
        recovered.fsck().unwrap();
    }
}

#[test]
fn checkout_abort_restores_original_files_refs_and_staging() {
    // Journal, three worktree writes, index, HEAD: interrupt before final log.
    for boundary in 1..=6 {
        let dir = tempdir().unwrap();
        let (_, second) = seed(dir.path());
        let backend = FaultBackend {
            inner: FsBackend::open(dir.path()).unwrap(),
            remaining: Cell::new(Some(boundary)),
        };
        let repo = Repository::open(backend).unwrap();
        assert!(
            repo.apply_checkout(repo.plan_checkout("v1").unwrap())
                .is_err()
        );
        drop(repo);
        let recovered = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
        recovered.recover(true).unwrap();
        assert_eq!(
            recovered.head_id().unwrap(),
            Some(second),
            "boundary {boundary}"
        );
        assert_eq!(recovered.status().unwrap().branch.as_deref(), Some("main"));
        assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "two");
        assert_eq!(fs::read_to_string(dir.path().join("new")).unwrap(), "new");
        assert!(!dir.path().join("old").exists());
        assert!(recovered.status().unwrap().unstaged.is_empty());
        recovered.fsck().unwrap();
    }
}

#[test]
fn recovery_does_not_overwrite_external_edits_and_blocks_unrelated_mutations() {
    let dir = tempdir().unwrap();
    seed(dir.path());
    let backend = FaultBackend {
        inner: FsBackend::open(dir.path()).unwrap(),
        remaining: Cell::new(Some(1)),
    };
    let repo = Repository::open(backend).unwrap();
    assert!(
        repo.apply_checkout(repo.plan_checkout("v1").unwrap())
            .is_err()
    );
    assert!(matches!(
        repo.create_issue("Blocked", ""),
        Err(Error::Pending)
    ));
    drop(repo);
    fs::write(dir.path().join("a"), "external edit after crash").unwrap();
    let recovered = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
    assert!(matches!(recovered.recover(false), Err(Error::Conflict(_))));
    assert!(matches!(recovered.recover(true), Err(Error::Conflict(_))));
    assert_eq!(
        fs::read_to_string(dir.path().join("a")).unwrap(),
        "external edit after crash"
    );
    assert!(
        recovered
            .backend()
            .read_meta("transaction.json")
            .unwrap()
            .is_some()
    );
}

#[test]
fn interrupted_issue_update_can_continue_or_abort() {
    for abort in [false, true] {
        let dir = tempdir().unwrap();
        let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
        let issue = repo.create_issue("Investigate fit", "").unwrap();
        drop(repo);
        let backend = FaultBackend {
            inner: FsBackend::open(dir.path()).unwrap(),
            remaining: Cell::new(Some(2)),
        };
        let repo = Repository::open(backend).unwrap();
        assert!(
            repo.update_issue(&issue.id.to_string(), IssueAction::Close, None)
                .is_err()
        );
        drop(repo);
        let recovered = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
        recovered.recover(abort).unwrap();
        assert_eq!(
            recovered.issue(&issue.id.to_string()).unwrap().status,
            if abort { "open" } else { "closed" }
        );
        recovered.fsck().unwrap();
    }
}

#[test]
fn finalized_transaction_cannot_be_reversed_after_its_completion_log() {
    let dir = tempdir().unwrap();
    seed(dir.path());
    // Journal + 3 worktree writes + index + HEAD + completion log.
    let backend = FaultBackend {
        inner: FsBackend::open(dir.path()).unwrap(),
        remaining: Cell::new(Some(7)),
    };
    let repo = Repository::open(backend).unwrap();
    assert!(
        repo.apply_checkout(repo.plan_checkout("v1").unwrap())
            .is_err()
    );
    drop(repo);
    let recovered = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
    assert!(matches!(recovered.recover(true), Err(Error::Conflict(_))));
    recovered.recover(false).unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "one");
}

#[test]
fn dependency_staging_recovers_or_aborts_without_changing_committed_history() {
    for (boundary, abort) in [
        (1, false),
        (2, false),
        (3, false),
        (4, false),
        (1, true),
        (2, true),
    ] {
        let dir = tempdir().unwrap();
        let (_, head) = seed(dir.path());
        let backend = FaultBackend {
            inner: FsBackend::open(dir.path()).unwrap(),
            remaining: Cell::new(Some(boundary)),
        };
        let repo = Repository::open(backend).unwrap();
        let _ = repo.add_dependency("a", "new", DependencyRequest::Revision(None));
        drop(repo);
        let recovered = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
        if recovered
            .backend()
            .read_meta("transaction.json")
            .unwrap()
            .is_some()
        {
            recovered.recover(abort).unwrap();
        }
        assert_eq!(
            recovered.graph(None).unwrap().edges.len(),
            if abort { 0 } else { 1 }
        );
        assert_eq!(recovered.head_id().unwrap(), Some(head));
        assert!(recovered.graph(Some("HEAD")).unwrap().edges.is_empty());
        assert!(recovered.graph(None).unwrap().requirements_satisfied);
        recovered.fsck().unwrap();
    }
}
