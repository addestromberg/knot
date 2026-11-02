#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::{Backend, Repository};
use knot_store::FsBackend;
use std::{fs, path::Path};
use tempfile::tempdir;

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn root_patterns_support_git_globs_anchors_negation_comments_and_escapes() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    write(
        root,
        ".knotignore",
        r"# Temporary engineering outputs

*.bak
!keep.bak
/root-only.txt
build/
artifacts/**/*.stl
log?.txt
sample[0-9].txt
\#private.txt
\!private.txt
drop.me
/blocked/
!/blocked/keep.txt
/salvage/*
!/salvage/keep.txt
!.knot/**
!.git/**
",
    );
    let ignored = [
        "part.bak",
        "nested/part.bak",
        "root-only.txt",
        "build/output.step",
        "nested/build/output.step",
        "artifacts/mesh.stl",
        "artifacts/deep/mesh.stl",
        "log1.txt",
        "sample7.txt",
        "#private.txt",
        "!private.txt",
        "drop.me",
        "blocked/keep.txt",
        "blocked/.knotignore",
        "salvage/drop.txt",
        ".git/credentials",
        "nested/.git/config",
    ];
    let kept = [
        "keep.bak",
        "nested/keep.bak",
        "nested/root-only.txt",
        "other/artifacts/mesh.stl",
        "log01.txt",
        "samplex.txt",
        "salvage/keep.txt",
        "git-only.txt",
        ".gitignore",
    ];
    for path in ignored.iter().chain(&kept) {
        write(root, path, "content");
    }
    // An excluded directory is pruned; its malformed ignore file is never read.
    fs::write(root.join("blocked/.knotignore"), b"\xff\n").unwrap();
    write(root, ".gitignore", "git-only.txt\n");
    let mut expected = kept.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    expected.push(".knotignore".into());
    expected.sort();
    assert_eq!(repo.status().unwrap().untracked, expected);
    repo.add(&[".".into()]).unwrap();
    let mut staged = repo
        .graph(None)
        .unwrap()
        .nodes
        .into_iter()
        .map(|n| n.path)
        .collect::<Vec<_>>();
    staged.sort();
    assert_eq!(staged, expected);
    repo.commit("Ignore generated outputs").unwrap();
    assert!(repo.status().unwrap().untracked.is_empty());
    repo.fsck().unwrap();
}

#[test]
fn nested_rules_are_relative_override_parents_and_do_not_leak_into_siblings() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    write(root, ".knotignore", "*.tmp\n!allow.tmp\n");
    write(
        root,
        "cad/.knotignore",
        "!keep.tmp\nallow.tmp\n/root-only.dat\n",
    );
    write(root, "cad/deep/.knotignore", "keep.tmp\n!deep.tmp\n");
    let ignored = [
        "root.tmp",
        "cad/drop.tmp",
        "cad/allow.tmp",
        "cad/root-only.dat",
        "cad/deep/keep.tmp",
        "cad/deep/drop.tmp",
        "sibling/keep.tmp",
    ];
    let kept = [
        "allow.tmp",
        "root-only.dat",
        "cad/keep.tmp",
        "cad/deep/deep.tmp",
        "cad/deep/root-only.dat",
        "sibling/allow.tmp",
        "sibling/root-only.dat",
    ];
    for path in ignored.iter().chain(&kept) {
        write(root, path, "content");
    }
    let mut expected = kept.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    expected.extend([
        ".knotignore".into(),
        "cad/.knotignore".into(),
        "cad/deep/.knotignore".into(),
    ]);
    expected.sort();
    assert_eq!(repo.status().unwrap().untracked, expected);
    repo.add(&[".".into()]).unwrap();
    assert_eq!(repo.graph(None).unwrap().nodes.len(), expected.len());
    repo.fsck().unwrap();
}

#[test]
fn ignored_tracked_files_still_report_and_stage_modifications_and_deletions() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    write(root, "cad/model.step", "R1");
    write(root, "cad/notes.md", "Notes");
    repo.add(&[".".into()]).unwrap();
    repo.commit("Track models").unwrap();
    write(root, ".knotignore", "/cad/\n");
    write(root, "cad/model.step", "R2");
    write(root, "cad/generated.stl", "Untracked mesh");
    fs::remove_file(root.join("cad/notes.md")).unwrap();
    let status = repo.status().unwrap();
    assert_eq!(status.untracked, vec![".knotignore"]);
    assert_eq!(status.unstaged.len(), 2);
    repo.add(&["cad".into()]).unwrap();
    let status = repo.status().unwrap();
    assert_eq!(status.staged.len(), 2);
    assert!(status.unstaged.is_empty());
    assert_eq!(status.dependency_graph.nodes.len(), 1);
    repo.commit("Update ignored tracked model").unwrap();
    assert_eq!(
        fs::read_to_string(root.join("cad/generated.stl")).unwrap(),
        "Untracked mesh"
    );
    repo.fsck().unwrap();
}

#[test]
fn nested_ignore_rules_never_allow_checkout_to_overwrite_local_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    repo.write_asset("base", b"Base").unwrap();
    let base = repo.commit("Base").unwrap();
    repo.write_asset("cad/model.step", b"Tracked model")
        .unwrap();
    repo.commit("Model").unwrap();
    repo.apply_checkout(repo.plan_checkout(base.as_str()).unwrap())
        .unwrap();
    write(root, "cad/.knotignore", "/model.step\n");
    write(root, "cad/model.step", "Precious local model");
    assert_eq!(repo.status().unwrap().untracked, vec!["cad/.knotignore"]);
    assert!(repo.plan_checkout("main").is_err());
    assert_eq!(repo.head_id().unwrap(), Some(base));
    assert_eq!(
        fs::read_to_string(root.join("cad/model.step")).unwrap(),
        "Precious local model"
    );
    assert!(
        repo.backend()
            .read_meta("transaction.json")
            .unwrap()
            .is_none()
    );
}

#[test]
fn invalid_nested_ignore_rules_fail_before_staging_changes() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    write(root, "part", "Model");
    let index = repo.status().unwrap().staged_snapshot;
    fs::create_dir(root.join("cad")).unwrap();
    fs::write(root.join("cad/.knotignore"), b"\xff\n").unwrap();
    let error = repo.add(&[".".into()]).unwrap_err();
    assert_eq!(error.code(), "invalid_input");
    assert!(error.to_string().contains(".knotignore"));
    assert_eq!(
        repo.backend().read_meta("index").unwrap(),
        Some(format!("{index}\n"))
    );
}

#[cfg(unix)]
#[test]
fn nested_ignore_files_never_follow_symbolic_links() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    write(outside.path(), "rules", "*.step\n");
    fs::create_dir(dir.path().join("cad")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("rules"),
        dir.path().join("cad/.knotignore"),
    )
    .unwrap();
    assert!(
        repo.status()
            .unwrap_err()
            .to_string()
            .contains("symbolic links")
    );
}
