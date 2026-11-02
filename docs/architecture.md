# Architecture decisions

Status: M1 and M2 implemented; readable repository formats 1 and 2. Changes to canonical bytes require an explicit format decision and compatibility tests.

## Backend and interface boundaries

All backend functionality is Rust. `knot-core` contains typed objects, portable path rules, structured errors, the `Backend` trait, and `Repository<B>` domain operations. `knot-store` implements that trait with a filesystem and holds a repository lock for its lifetime. `knot-cli` parses commands, resolves operation-relative paths, and renders results. The future MCP server, server APIs, and React frontend must reuse the domain operations.

Three crates provide enough separation without the proposed eight mostly empty crates. The M2 graph stays in `knot-core` to reuse staging and integrity operations; resolver and validation crates can be extracted when those engines exist. Synchronous I/O avoids requiring an asynchronous runtime in local core operations; a future server may place it behind worker boundaries.

## Hidden contradictions resolved

1. **Identity versus filename/content:** a UUIDv4-bearing Asset object establishes identity before dependency graph work. Its hash is stable; path changes live in snapshots. Copies intentionally receive another Asset object. Rename inference is unreliable, particularly when files are edited simultaneously, so M1 requires `mv` for identity preservation.
2. **Determinism versus historical metadata:** snapshots contain only sorted paths and exact revisions. Revision objects omit timestamps. Commits include author, time, message, and parents, so different commits may reference the same snapshot. Independently initialized repositories are not expected to generate identical random asset identities.
3. **File hash versus object ID:** the conceptual SHA-256(file-bytes) formula is replaced with a type/version/length-separated envelope for every stored object, including blobs. Identical blob bytes still deduplicate. The full SHA-256 digest identifies an exact revision; labels such as R10 are not globally meaningful identities.
4. **Reconstructable indexes versus uncommitted staging:** `.knot/index` is an authoritative pointer to an immutable snapshot. It is not a disposable derived index. Lookup indexes and caches may be discarded; objects, refs, staging, configuration, and recovery records must be preserved.
5. **Validation circularity:** validation evidence must reference the input snapshot, never become part of that snapshot. Future resolution proves only declared compatibility and must report unknown checks; it does not itself validate an engineering design.
6. **Issue history versus engineering branches:** issues have independent event chains under `refs/issues/`. Returning to an old CAD/firmware snapshot must not rewind current engineering discussion. These events can be exchanged through future object/ref synchronization without a database.
7. **Atomic file writes versus whole-tree checkout:** a tree cannot be atomically replaced file by file. Journaled operations therefore store complete before/after fingerprints and metadata values, retain immutable backup blobs, and support guarded continuation or rollback.

## Consistency and concurrency

The filesystem backend acquires an exclusive nonblocking OS file lock for every session, including read sessions. A competing process receives exit code 5. This deliberately simple policy favors correctness over concurrent reader throughput. Locks are released by the OS when a process exits; stale lock-file deletion is unnecessary.

Initialization builds metadata in a reserved `.knot-init-*` temporary directory before publishing the complete `.knot/` directory. A failed pre-publication initialization does not expose a partial repository; ordinary failures clean up staging automatically. A killed process may leave its private initialization directory, which worktree discovery excludes.

Objects are written and synced before publication. Existing object paths are verified and never replaced. Mutable pointers use temporary files in their destination directory and atomic replacement. Every multi-file mutation writes a versioned journal before changing files or refs. Expected values prevent stale updates; a checkout plan is recomputed before application.

A transaction applies deletions before additions to support file/directory transitions, then metadata updates, then a completed audit log, and finally removes its journal. Recovery accepts only recorded before/after file states. Once a completion log exists, its outcome cannot be reversed by choosing abort. Validation happens before publishing snapshots so a rejected staging operation does not leave an invalid snapshot object.

Filesystem locks coordinate cooperating Knot processes. The implementation rejects symlinks observed in paths but does not claim isolation against hostile same-user filesystem changes between checks and opens. Use local filesystems; network filesystem locking and power-loss durability are not guaranteed. Unix directory fsync and file fsync improve durability; Windows directory durability remains dependent on its filesystem APIs.

## Scope and tradeoffs

M1 supports portable regular files and stable path casing. Case-only path transitions require an intermediate name and snapshot; this avoids divergent behavior on case-insensitive hosts. No automatic line-ending conversion, semantic format interpretation, binary merging, garbage collection, authentication, or external test execution occurs.

Blobs stream in 64 KiB buffers. Buffered object reads and structured objects are limited to 16 MiB. Text diff generation is limited to 1 MiB per side; larger assets receive hash/size summaries. Whole-file deduplication is implemented; block deduplication and compression are deferred.

A copied `.knot/` contains everything needed to reconstruct tracked bytes with `restore .`. Author configuration and issue history are copied too. Untracked files are outside the reproducibility guarantee.

## Acceptance progression

M1 verifies storage, identity, staging, history, branches/tags, checkout, recovery, JSON interfaces, and independent issues. The electromechanical test fixture uses CAD/PCB-like binary bytes plus firmware source; it does not claim geometric interpretation.

M2 now adds stable-ID dependency/interface objects, exact requirement checks, cycle-safe traversal, reverse queries, and contract-aware impact analysis. Its graph follows immutable revision references, not mutable global configuration. M3 adds deterministic resolution, previews, and structured conflicts. M4 adds exact-input validation evidence and documentation traceability. Only then can the complete housing–PCB–firmware compatibility scenario be considered implemented. M5 adds MCP/Skills; M6 transfers objects/refs; M7 adds hosting and React.

## M2 decisions

Engineering metadata belongs to the owner Revision. A dependency or export change therefore creates a metadata revision even when bytes are identical; it does not modify an existing revision. Repeated staging retains committed ancestry, and `add` preserves staged declarations. Snapshot/Commit encodings and all M1 golden hashes stay unchanged. Revision schema 2 and a conservative repository capability marker provide explicit forward incompatibility for old readers.

M2 checks exact declared revision hashes and exact interface versions. Range solving and inferred physical compatibility are deferred. Interface definitions are explicit opaque bytes, so unchanged contracts can survive a content revision; they are user-maintained declarations, not validation evidence. Default commit/checkout reject unsatisfied declarations, while explicit `--allow-incompatible` records/restores WIP without weakening integrity or dirty-file protections.

Graph queries default to staging and accept committed historical views. Impact without a baseline reports potential reverse reachability; `--since` compares actual selected revisions and interface exports, suppressing unchanged direct contracts. Downstream propagation is conservatively reported for review. No test execution, stale-evidence decisions, or automatic compatibility resolution is claimed.

## Verification direction

Prioritize manual review records and dependency-change warnings before automated engineering validators. Review applicability is derived from exact recorded inputs, while downstream warnings identify unresolved upstream reviews. Matching declarations and human verification are separate states. No agent or extractor is required, and unchanged interface declarations alone must not suppress manual review of changed endpoint revisions. Evidence stays outside engineering state hashes. See [the planned workflow](engineering-graph.md#planned-manual-verification-workflow); this does not change current M2 behavior.
