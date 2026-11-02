#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use serde_json::json;

#[test]
fn object_hashes_match_independent_golden_vectors() {
    assert_eq!(
        object_header(ObjectKind::Blob, 6),
        b"knot\0blob\x001\x006\0"
    );
    assert_eq!(
        object_id(ObjectKind::Blob, b"hello\n").as_str(),
        "ba0990d3338beef2f68c2f9329ea73f255984e23d32aa8489a2b8ceedc45c5bb"
    );
    let bytes = canonical(&Snapshot::default()).unwrap();
    assert_eq!(bytes, b"{\"entries\":[]}");
    assert_eq!(
        object_id(ObjectKind::Snapshot, &bytes).as_str(),
        "d6ee12f36aea8615b08be863465ef2955755a3a6d4aecfc693f788670fd1d22b"
    );
    assert_ne!(
        object_id(ObjectKind::Blob, &bytes),
        object_id(ObjectKind::Snapshot, &bytes)
    );
}

#[test]
fn canonical_json_obeys_utf16_key_order_and_escapes() {
    let data = json!({"\u{e000}": "private", "\u{10000}": "astral", "a": "line\n\"quote\""});
    let encoded = String::from_utf8(canonical(&data).unwrap()).unwrap();
    assert_eq!(
        encoded,
        "{\"a\":\"line\\n\\\"quote\\\"\",\"\u{10000}\":\"astral\",\"\u{e000}\":\"private\"}"
    );
}

#[test]
fn object_ids_reject_noncanonical_strings() {
    for value in ["", "sha256:abc", &"A".repeat(64), &"g".repeat(64)] {
        assert!(value.parse::<ObjectId>().is_err());
    }
    let id = object_id(ObjectKind::Blob, b"hi");
    assert_eq!(
        serde_json::from_str::<ObjectId>(&serde_json::to_string(&id).unwrap()).unwrap(),
        id
    );
}

#[test]
fn portable_paths_reject_traversal_devices_and_ambiguity() {
    for path in [
        "../x",
        "/x",
        "a//b",
        "a/./b",
        "a\\b",
        "a:b",
        "NUL.txt",
        "com1",
        "LPT¹",
        "end.",
        "end ",
        ".knot/HEAD",
        "a/.git/index",
        "e\u{301}",
    ] {
        assert!(validate_path(path).is_err(), "accepted {path:?}");
    }
    for path in [
        "mechanical/housing.FCStd",
        "docs/Ångström.md",
        ".knotignore",
        "firmware/main.rs",
    ] {
        validate_path(path).unwrap();
    }
    assert!(validate_path_set(["a.txt", "A.txt"]).is_err());
    assert!(validate_path_set(["Straße", "STRASSE"]).is_err());
    assert!(validate_path_set(["folder", "folder/file"]).is_err());
    assert!(validate_ref_name("feature/new-housing").is_ok());
    for name in ["HEAD", ".hidden", "a.lock", "a..b", "a@b"] {
        assert!(validate_ref_name(name).is_err());
    }
}

#[test]
fn directory_components_have_consistent_case_across_all_entries() {
    assert!(validate_path_set(["Mechanical/a.step", "mechanical/b.step"]).is_err());
    assert!(validate_path_set(["Straße/a", "STRASSE/b"]).is_err());
    validate_path_set(["Mechanical/a.step", "Mechanical/b.step"]).unwrap();
}

#[test]
fn typed_models_reject_noncanonical_metadata_and_duplicate_parents() {
    let id = object_id(ObjectKind::Blob, b"x");
    let mut commit = Commit {
        snapshot: id.clone(),
        parents: vec![],
        message: "Test".into(),
        author: None,
        timestamp: "01700000000".into(),
    };
    assert!(commit.validate().is_err());
    commit.timestamp = "1700000000".into();
    commit.parents = vec![id.clone(), id.clone()];
    assert!(commit.validate().is_err());
    let event = IssueEvent {
        issue: uuid::Uuid::new_v4(),
        parent: None,
        action: IssueAction::Close,
        author: None,
        timestamp: "1700000000".into(),
    };
    assert!(event.validate().is_err());
    let revision = Revision {
        asset: id.clone(),
        blob: id.clone(),
        parents: vec![id.clone(), id],
        executable: false,
        dependencies: vec![],
        interfaces: vec![],
    };
    assert!(revision.validate().is_err());
}

#[test]
fn engineering_revision_schema_preserves_legacy_bytes_and_separates_versions() {
    let id = object_id(ObjectKind::Blob, b"x");
    let mut revision = Revision {
        asset: id.clone(),
        blob: id.clone(),
        parents: vec![],
        executable: false,
        dependencies: vec![],
        interfaces: vec![],
    };
    assert_eq!(revision.schema_version(), 1);
    let legacy = String::from_utf8(canonical(&revision).unwrap()).unwrap();
    assert!(!legacy.contains("dependencies"));
    assert!(!legacy.contains("interfaces"));
    revision.dependencies = vec![id];
    assert_eq!(revision.schema_version(), 2);
    let payload = canonical(&revision).unwrap();
    assert_ne!(
        object_id_versioned(ObjectKind::Revision, 1, &payload),
        object_id_versioned(ObjectKind::Revision, 2, &payload)
    );
    revision.dependencies.push(revision.dependencies[0].clone());
    assert!(revision.validate().is_err());
    for name in ["", "../mount", "mount/holes", "two words", "5pins"] {
        assert!(validate_interface_name(name).is_err());
    }
    for name in ["pcb_mount", "SPI.v1", "_internal", "usb-c"] {
        validate_interface_name(name).unwrap();
    }
}

#[test]
fn engineering_snapshot_matches_independently_computed_golden_hashes() {
    let asset = |identity: &str| {
        object_id(
            ObjectKind::Asset,
            &canonical(&Asset {
                identity: uuid::Uuid::parse_str(identity).unwrap(),
            })
            .unwrap(),
        )
    };
    let model = asset("00000000-0000-4000-8000-000000000001");
    let pcb = asset("00000000-0000-4000-8000-000000000002");
    let interface = Interface {
        asset: model.clone(),
        name: "pcb_mount".into(),
        definition: object_id(ObjectKind::Blob, b"mount=20mm\n"),
    };
    let interface = object_id(ObjectKind::Interface, &canonical(&interface).unwrap());
    assert_eq!(
        interface.as_str(),
        "c7c88a234b34c844649c71f7b324694e826da81f0eb90cca408f40bd328e1ddd"
    );
    let model = Revision {
        asset: model.clone(),
        blob: object_id(ObjectKind::Blob, b"housing R5"),
        parents: vec![],
        executable: false,
        dependencies: vec![],
        interfaces: vec![interface.clone()],
    };
    let model_id = object_id_versioned(ObjectKind::Revision, 2, &canonical(&model).unwrap());
    assert_eq!(
        model_id.as_str(),
        "7640265e65c7547bd6624eeb73486feee51b455f02feae1121a10ca84455b86c"
    );
    let dependency = Dependency {
        dependent: pcb.clone(),
        target: model.asset,
        requirement: DependencyRequirement::Interface {
            name: "pcb_mount".into(),
            interface,
        },
    };
    let dependency = object_id(ObjectKind::Dependency, &canonical(&dependency).unwrap());
    assert_eq!(
        dependency.as_str(),
        "4c1ad22f1ea99cfeeaed5b07c2eac1011c93208e26a3a4083229497b1dc29b76"
    );
    let pcb = Revision {
        asset: pcb,
        blob: object_id(ObjectKind::Blob, b"PCB R3"),
        parents: vec![],
        executable: false,
        dependencies: vec![dependency],
        interfaces: vec![],
    };
    let pcb_id = object_id_versioned(ObjectKind::Revision, 2, &canonical(&pcb).unwrap());
    let snapshot = Snapshot {
        entries: vec![
            SnapshotEntry {
                path: "housing.FCStd".into(),
                revision: model_id,
            },
            SnapshotEntry {
                path: "pcb".into(),
                revision: pcb_id,
            },
        ],
    };
    assert_eq!(
        object_id(ObjectKind::Snapshot, &canonical(&snapshot).unwrap()).as_str(),
        "cb6841fcaac809de65f6f22c8ad7964f6c6ca05831b366468ff529fc7cef3be9"
    );
}
