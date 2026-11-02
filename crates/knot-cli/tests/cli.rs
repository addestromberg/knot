#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::tempdir;

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}
fn ok(root: &Path, args: &[&str]) -> Value {
    let result = run(root, args);
    assert!(
        result.status.success(),
        "{args:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let data: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["schema_version"], 1);
    assert_eq!(data["ok"], true);
    assert!(result.stderr.is_empty());
    data["data"].clone()
}

fn human(root: &Path, args: &[&str]) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn status_displays_staged_graph_and_warns_about_unstaged_edits() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    let empty = human(root, &["status"]);
    assert!(empty.contains("On branch main · no commits yet"));
    assert!(empty.contains("No tracked files"));
    for (path, body) in [("pipe.step", "pipe R1"), ("rod.step", "rod R1")] {
        fs::write(root.join(path), body).unwrap();
    }
    ok(root, &["add", "."]);
    ok(root, &["depend", "add", "rod.step", "--on", "pipe.step"]);
    ok(root, &["commit", "-m", "Rod and pipe"]);
    let before = ok(root, &["status"]);
    let clean = human(root, &["status"]);
    assert!(clean.contains("Working tree clean"));
    assert!(clean.contains("Dependency graph (staged; parent depends on child)"));
    assert!(clean.contains("#2 rod.step @"));
    assert!(clean.contains("└── #1 pipe.step @"));
    assert!(clean.contains("[matches]"));
    assert!(clean.contains("2 tracked files · 1 dependency · 0 conflicts"));
    assert!(!clean.contains('\u{1b}'));
    assert_eq!(ok(root, &["status"]), before);

    fs::write(root.join("pipe.step"), "pipe with mount").unwrap();
    fs::write(root.join("notes.txt"), "review mounting clearance").unwrap();
    let unstaged = human(root, &["status"]);
    assert!(unstaged.contains("Unstaged changes:"));
    assert!(unstaged.contains("Untracked files:\n  ?  notes.txt"));
    assert!(unstaged.contains("[unstaged modified]"));
    assert!(unstaged.contains("unstaged edits are not included in dependency checks"));
    assert_eq!(ok(root, &["status"])["requirements_satisfied"], true);

    ok(root, &["add", "pipe.step"]);
    let staged = human(root, &["status"]);
    assert!(staged.contains("Staged changes:"));
    assert!(staged.contains("[staged]"));
    assert!(staged.contains("[REVISION MISMATCH]"));
    assert!(staged.contains("1 conflict"));
    assert!(staged.contains("unsatisfied requirements block ordinary commits"));
    assert!(!staged.contains("[unstaged modified]"));
    let data = ok(root, &["status"]);
    assert_eq!(data["head"], before["head"]);
    assert_eq!(data["requirements_satisfied"], false);
    assert_eq!(data["dependency_graph"], ok(root, &["graph"]));
    assert_eq!(
        data["dependency_conflicts"][0],
        data["dependency_graph"]["edges"][0]
    );
}

#[test]
fn graph_renders_shared_dependencies_once_and_missing_targets_explicitly() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    for name in ["pipe.step", "rod.step", "sleeve.step", "readme.txt"] {
        fs::write(root.join(name), name).unwrap();
    }
    ok(root, &["add", "."]);
    for owner in ["rod.step", "sleeve.step"] {
        ok(root, &["depend", "add", owner, "--on", "pipe.step"]);
    }
    ok(root, &["commit", "-m", "Shared pipe dependency"]);
    let output = human(root, &["graph"]);
    for name in ["pipe.step", "rod.step", "sleeve.step", "readme.txt"] {
        assert_eq!(output.matches(name).count(), 1, "{output}");
    }
    assert!(output.contains("↳ #1 (already shown)"));
    assert_eq!(output.matches("[matches]").count(), 2);
    ok(root, &["rm", "pipe.step"]);
    let missing = human(root, &["status"]);
    assert_eq!(missing.matches("[MISSING ASSET]").count(), 2);
    assert!(missing.contains("3 tracked files · 2 dependencies · 2 conflicts"));
}

#[test]
fn interface_cycles_render_finitely_and_show_changed_contract_versions() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    for (name, content) in [
        ("pipe.step", "pipe"),
        ("rod.step", "rod"),
        ("fit.txt", "20 mm"),
    ] {
        fs::write(root.join(name), content).unwrap();
    }
    ok(root, &["add", "."]);
    for asset in ["pipe.step", "rod.step"] {
        ok(
            root,
            &[
                "interface",
                "set",
                asset,
                "--name",
                "fit",
                "--file",
                "fit.txt",
            ],
        );
    }
    for (owner, target) in [("pipe.step", "rod.step"), ("rod.step", "pipe.step")] {
        ok(
            root,
            &["depend", "add", owner, "--on", target, "--interface", "fit"],
        );
    }
    ok(root, &["commit", "-m", "Mutual fit declarations"]);
    let output = human(root, &["status"]);
    assert!(output.contains("(cycle)"));
    assert_eq!(output.matches("pipe.step").count(), 1);
    assert_eq!(output.matches("rod.step").count(), 1);
    assert_eq!(output.matches("[matches]").count(), 2);
    assert!(output.contains("requires fit@"));
    fs::write(root.join("fit.txt"), "25 mm").unwrap();
    ok(
        root,
        &[
            "interface",
            "set",
            "pipe.step",
            "--name",
            "fit",
            "--file",
            "fit.txt",
        ],
    );
    let changed = human(root, &["status"]);
    assert!(changed.contains("[INTERFACE MISMATCH]; selected interface @"));
    ok(root, &["interface", "remove", "pipe.step", "--name", "fit"]);
    assert!(human(root, &["status"]).contains("[MISSING INTERFACE]"));
}

#[test]
fn directories_contain_only_selected_versions_across_history_and_renames() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    let mechanical = root.join("mechanical");
    fs::create_dir(&mechanical).unwrap();
    fs::write(mechanical.join("pipe.step"), "pipe R1").unwrap();
    fs::write(mechanical.join("rod.step"), "rod R1").unwrap();
    ok(root, &["add", "."]);
    ok(
        root,
        &[
            "depend",
            "add",
            "mechanical/rod.step",
            "--on",
            "mechanical/pipe.step",
        ],
    );
    ok(root, &["commit", "-m", "Initial assembly"]);
    ok(root, &["tag", "original"]);
    for revision in 2..=4 {
        fs::write(mechanical.join("pipe.step"), format!("pipe R{revision}")).unwrap();
        ok(root, &["add", "mechanical/pipe.step"]);
        ok(
            root,
            &[
                "depend",
                "add",
                "mechanical/rod.step",
                "--on",
                "mechanical/pipe.step",
            ],
        );
        ok(root, &["commit", "-m", "Reviewed updated assembly"]);
    }
    ok(
        root,
        &["mv", "mechanical/pipe.step", "mechanical/tube.step"],
    );
    ok(root, &["commit", "-m", "Rename pipe"]);
    for (target, name, content) in [
        ("original", "pipe.step", "pipe R1"),
        ("main", "tube.step", "pipe R4"),
        ("original", "pipe.step", "pipe R1"),
    ] {
        ok(root, &["checkout", target]);
        let mut files: Vec<_> = fs::read_dir(&mechanical)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        let mut expected = vec!["rod.step".to_owned(), name.to_owned()];
        expected.sort();
        assert_eq!(files, expected);
        assert_eq!(fs::read_to_string(mechanical.join(name)).unwrap(), content);
        assert_eq!(
            fs::read_to_string(mechanical.join("rod.step")).unwrap(),
            "rod R1"
        );
        assert_eq!(
            ok(root, &["status"])["dependency_graph"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    assert!(human(root, &["status"]).contains("Detached HEAD"));
    assert_eq!(ok(root, &["log"]).as_array().unwrap().len(), 1);
    assert_eq!(
        ok(root, &["log", "--target", "main"])
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn cli_versioning_and_issue_workflow() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init", "--author", "A Local Engineer"]);
    fs::write(root.join("housing.FCStd"), b"CAD\0R1\xff").unwrap();
    ok(root, &["add", "."]);
    let first = ok(root, &["commit", "-m", "Initial housing"])["commit"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(root, &["tag", "v1"]);
    ok(root, &["branch", "feature/new-housing"]);
    let issue = ok(
        root,
        &[
            "issue",
            "create",
            "--title",
            "Adjust mounting holes",
            "--body",
            "Check fit",
        ],
    );
    let id = issue["id"].as_str().unwrap();
    let head = issue["head"].as_str().unwrap();
    ok(root, &["issue", "link", id, "--commit", &first]);
    let stale = run(root, &["issue", "close", id, "--expected-head", head]);
    assert_eq!(stale.status.code(), Some(3));
    fs::write(root.join("comment.md"), "Measured **20 mm**.\n").unwrap();
    ok(root, &["issue", "comment", id, "--body-file", "comment.md"]);
    ok(root, &["issue", "link", id, "--asset", "housing.FCStd"]);
    ok(root, &["issue", "close", id]);
    assert_eq!(ok(root, &["issue", "list"]), json!([]));
    assert_eq!(
        ok(root, &["issue", "list", "--all"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::write(root.join("housing.FCStd"), b"CAD\0R2\xfe").unwrap();
    ok(root, &["add", "housing.FCStd"]);
    ok(root, &["commit", "-m", "Update housing"]);
    let before = fs::read(root.join("housing.FCStd")).unwrap();
    ok(root, &["checkout", "v1", "--preview"]);
    assert_eq!(fs::read(root.join("housing.FCStd")).unwrap(), before);
    ok(root, &["checkout", "v1"]);
    assert_eq!(
        fs::read(root.join("housing.FCStd")).unwrap(),
        b"CAD\0R1\xff"
    );
    assert_eq!(ok(root, &["issue", "show", id])["status"], "closed");
    ok(root, &["checkout", "main"]);
    ok(root, &["fsck"]);
    assert_eq!(ok(root, &["log"]).as_array().unwrap().len(), 2);
}

#[test]
fn json_errors_have_stable_exit_codes_and_clean_stdout() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    for args in [
        vec!["missing-command"],
        vec!["commit"],
        vec!["recover"],
        vec!["issue", "create"],
    ] {
        let output = run(root, &args);
        assert_eq!(output.status.code(), Some(2));
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["ok"], false);
        assert_eq!(error["error"]["code"], "invalid_input");
        assert!(output.stderr.is_empty());
    }
    assert_eq!(run(root, &["status"]).status.code(), Some(1));
    ok(root, &["init"]);
    assert_eq!(run(root, &["init"]).status.code(), Some(3));
    fs::write(root.join("a"), "one\n").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "One"]);
    fs::write(root.join("a"), "dirty\n").unwrap();
    assert_eq!(run(root, &["checkout", "main"]).status.code(), Some(3));
    let diff = ok(root, &["diff"]);
    assert!(diff[0]["patch"].as_str().unwrap().contains("+dirty"));
    ok(root, &["restore", "a"]);
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "one\n");
}

#[test]
fn subdirectory_paths_are_relative_to_operation_directory() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::create_dir(root.join("mechanical")).unwrap();
    fs::write(root.join("mechanical/a.step"), "cad").unwrap();
    fs::write(root.join("untracked"), "leave alone").unwrap();
    ok(&root.join("mechanical"), &["add", "."]);
    let status = ok(root, &["status"]);
    assert_eq!(status["staged"][0]["path"], "mechanical/a.step");
    assert_eq!(status["untracked"], json!(["untracked"]));
    ok(root, &["commit", "-m", "Mechanical only"]);
    ok(&root.join("mechanical"), &["mv", "a.step", "b.step"]);
    ok(root, &["commit", "-m", "Rename"]);
    assert_eq!(
        ok(&root.join("mechanical"), &["log", "b.step"])
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(run(root, &["add", "../escape"]).status.code(), Some(2));
}

#[test]
fn lock_is_respected_by_a_separate_cli_process() {
    use knot_store::FsBackend;
    let dir = tempdir().unwrap();
    let backend = FsBackend::init(dir.path(), None).unwrap();
    let output = run(dir.path(), &["status"]);
    assert_eq!(output.status.code(), Some(5));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "repository_busy");
    drop(backend);
    ok(dir.path(), &["status"]);
}

#[test]
fn dependency_commands_report_conflicts_and_explicit_wip_overrides() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::write(root.join("housing.FCStd"), "housing R5").unwrap();
    fs::write(root.join("pcb"), "PCB R3").unwrap();
    fs::write(root.join("firmware"), "firmware").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Assets"]);
    let model = ok(root, &["deps", "housing.FCStd"])["root"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        root,
        &[
            "depend",
            "add",
            "pcb",
            "--on",
            "housing.FCStd",
            "--revision",
            &model,
        ],
    );
    ok(root, &["depend", "add", "firmware", "--on", "pcb"]);
    let diff = ok(root, &["diff", "--staged"]);
    assert!(
        diff.as_array()
            .unwrap()
            .iter()
            .all(|d| d["metadata_changes"] == json!(["constraints"]))
    );
    ok(root, &["commit", "-m", "Declared pins"]);
    ok(root, &["tag", "valid"]);
    fs::write(root.join("housing.FCStd"), "housing R6").unwrap();
    ok(root, &["add", "housing.FCStd"]);
    let graph = ok(root, &["graph"]);
    assert_eq!(graph["requirements_satisfied"], false);
    assert_eq!(graph["declarations_only"], true);
    assert_eq!(
        ok(root, &["status"])["dependency_conflicts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let impact = ok(root, &["impact", "housing.FCStd", "--since", "HEAD"]);
    assert_eq!(impact["direct"][0]["path"], "pcb");
    assert_eq!(impact["transitive"][0]["path"], "firmware");
    let failed = run(root, &["commit", "-m", "Unsatisfied"]);
    assert_eq!(failed.status.code(), Some(3));
    let error: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(error["error"]["code"], "dependency_conflict");
    assert_eq!(
        error["error"]["details"]["conflicts"][0]["status"],
        "revision_mismatch"
    );
    assert_eq!(
        error["error"]["details"]["conflicts"][0]["requirement"]["revision"],
        model
    );
    ok(
        root,
        &["commit", "-m", "Explicit WIP", "--allow-incompatible"],
    );
    ok(root, &["checkout", "valid"]);
    assert_eq!(run(root, &["checkout", "main"]).status.code(), Some(3));
    ok(
        root,
        &["checkout", "main", "--allow-incompatible", "--preview"],
    );
    ok(root, &["checkout", "main", "--allow-incompatible"]);
    ok(root, &["fsck"]);
    assert_eq!(
        ok(root, &["graph", "--at", "valid"])["requirements_satisfied"],
        true
    );
}

#[test]
fn interface_cli_distinguishes_unchanged_and_changed_contracts() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    for (file, body) in [
        ("housing", "CAD"),
        ("pcb", "PCB"),
        ("mount.txt", "holes 20mm"),
    ] {
        fs::write(root.join(file), body).unwrap();
    }
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Assets"]);
    ok(
        root,
        &[
            "interface",
            "set",
            "housing",
            "--name",
            "pcb_mount",
            "--file",
            "mount.txt",
        ],
    );
    let interface = ok(root, &["interface", "list", "housing"]);
    let asset = interface["asset"].as_str().unwrap();
    ok(
        root,
        &[
            "depend",
            "add",
            "pcb",
            "--on",
            asset,
            "--interface",
            "pcb_mount",
        ],
    );
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Interface contract"]);
    fs::write(root.join("housing"), "CAD cosmetic change").unwrap();
    ok(root, &["add", "housing"]);
    assert_eq!(ok(root, &["graph"])["requirements_satisfied"], true);
    assert_eq!(
        ok(root, &["impact", "housing", "--since", "HEAD"])["unaffected_direct"][0]["path"],
        "pcb"
    );
    ok(root, &["commit", "-m", "Same contract"]);
    fs::write(root.join("mount.txt"), "holes 25mm").unwrap();
    ok(
        root,
        &[
            "interface",
            "set",
            "housing",
            "--name",
            "pcb_mount",
            "--file",
            "mount.txt",
        ],
    );
    assert_eq!(
        ok(root, &["deps", "pcb"])["edges"][0]["status"],
        "interface_mismatch"
    );
    assert_eq!(
        ok(root, &["deps", "housing", "--reverse", "--transitive"])["edges"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    ok(
        root,
        &[
            "depend",
            "remove",
            "pcb",
            "--on",
            "housing",
            "--interface",
            "pcb_mount",
        ],
    );
    ok(
        root,
        &["interface", "remove", "housing", "--name", "pcb_mount"],
    );
    assert!(ok(root, &["graph"])["edges"].as_array().unwrap().is_empty());
    assert_eq!(
        run(
            root,
            &[
                "depend",
                "add",
                "pcb",
                "--on",
                "housing",
                "--interface",
                "pcb_mount",
                "--revision",
                &"0".repeat(64)
            ]
        )
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn dependency_paths_and_definition_files_work_from_subdirectories() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::create_dir(root.join("mechanical")).unwrap();
    fs::create_dir(root.join("electronics")).unwrap();
    fs::write(root.join("mechanical/housing"), "CAD").unwrap();
    fs::write(root.join("mechanical/mount.txt"), "holes 20mm").unwrap();
    fs::write(root.join("electronics/pcb"), "PCB").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Assets"]);
    ok(
        &root.join("mechanical"),
        &[
            "interface",
            "set",
            "housing",
            "--name",
            "mount",
            "--file",
            "mount.txt",
        ],
    );
    ok(
        &root.join("electronics"),
        &[
            "depend",
            "add",
            "pcb",
            "--on",
            "../mechanical/housing",
            "--interface",
            "mount",
        ],
    );
    let report = ok(&root.join("electronics"), &["deps", "pcb"]);
    assert_eq!(report["edges"][0]["target_path"], "mechanical/housing");
    assert_eq!(
        ok(
            &root.join("electronics"),
            &["impact", "../mechanical/housing"]
        )["direct"][0]["path"],
        "electronics/pcb"
    );
    ok(root, &["commit", "-m", "Declared interface"]);
    ok(root, &["fsck"]);
}
