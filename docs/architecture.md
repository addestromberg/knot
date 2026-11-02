# Architecture decisions

Status: M1–M6 implemented; readable repository capability markers 1, 2, and 3. Changes to canonical bytes require an explicit format decision and compatibility tests.

## Backend and interface boundaries

All backend functionality is Rust. `knot-core` contains typed objects, portable path rules, structured errors, the `Backend` trait, and `Repository<B>` domain operations. `knot-store` implements that trait with a filesystem and holds a repository lock for its lifetime. `knot-cli` parses commands, resolves operation-relative paths, and renders results. `knot-mcp` uses those same domain operations. The product is CLI-only; the hosting/React proposal has been removed.

Worktree discovery reads `.knotignore` files from the repository root down through
visited directories, using Git-compatible patterns with the nearest matching
file taking precedence. Excluded directories are pruned. Tracked-file checks and
checkout collision detection bypass ignores so rules cannot conceal modified
tracked files or authorize overwriting local data. Git/user-global ignore sources
are not imported; versioned `.knotignore` files keep shared rules reproducible.

Four crates provide the domain, filesystem/transports, CLI, and MCP boundaries. Graph, resolution, verification, and ref-publication logic stay in `knot-core`; filesystem/SSH/mesh adapters stay in `knot-store`. Synchronous I/O avoids requiring an asynchronous runtime in local core operations; a future server may place it behind worker boundaries.

## Hidden contradictions resolved

1. **Identity versus filename/content:** a UUIDv4-bearing Asset object establishes identity before dependency graph work. Its hash is stable; path changes live in snapshots. Copies intentionally receive another Asset object. Rename inference is unreliable, particularly when files are edited simultaneously, so M1 requires `mv` for identity preservation.
2. **Determinism versus historical metadata:** snapshots contain only sorted paths and exact revisions. Revision objects omit timestamps. Commits include author, time, message, and parents, so different commits may reference the same snapshot. Independently initialized repositories are not expected to generate identical random asset identities.
3. **File hash versus object ID:** the conceptual SHA-256(file-bytes) formula is replaced with a type/version/length-separated envelope for every stored object, including blobs. Identical blob bytes still deduplicate. The full SHA-256 digest identifies an exact revision; labels such as R10 are not globally meaningful identities.
4. **Reconstructable indexes versus uncommitted staging:** `.knot/index` is an authoritative pointer to an immutable snapshot. It is not a disposable derived index. Lookup indexes and caches may be discarded; objects, refs, staging, configuration, and recovery records must be preserved.
5. **Validation circularity:** validation evidence must reference the input snapshot, never become part of that snapshot. Resolution proves only declared compatibility and must report unknown checks; it does not itself validate an engineering design.
6. **Issue history versus engineering branches:** issues have independent event chains under `refs/issues/`. Returning to an old CAD/firmware snapshot must not rewind current engineering discussion. These events are exchanged through object/ref synchronization without a database.
7. **Atomic file writes versus whole-tree checkout:** a tree cannot be atomically replaced file by file. Journaled operations therefore store complete before/after fingerprints and metadata values, retain immutable backup blobs, and support guarded continuation or rollback.

## Consistency and concurrency

The filesystem backend acquires an exclusive nonblocking OS file lock for every session, including read sessions. A competing process receives exit code 5. This deliberately simple policy favors correctness over concurrent reader throughput. Locks are released by the OS when a process exits; stale lock-file deletion is unnecessary.

Initialization builds metadata in a reserved `.knot-init-*` temporary directory before publishing the complete `.knot/` directory. A failed pre-publication initialization does not expose a partial repository; ordinary failures clean up staging automatically. A killed process may leave its private initialization directory, which worktree discovery excludes.

Initialization closes the staging lock handle before renaming the directory,
since open child handles block publication on Windows. The fully written regular
or bare repository is then reopened and locked at its final path. A competing
process may acquire that lock first; in that case initialization returns busy
while leaving a complete repository, rather than exposing partial state.

Objects are written and synced before publication. Existing object paths are verified and never replaced. Mutable pointers use temporary files in their destination directory and atomic replacement. Every multi-file mutation writes a versioned journal before changing files or refs. Expected values prevent stale updates; a checkout plan is recomputed before application.

A transaction applies deletions before additions to support file/directory transitions, then metadata updates, then a completed audit log, and finally removes its journal. Recovery accepts only recorded before/after file states. Once a completion log exists, its outcome cannot be reversed by choosing abort. Validation happens before publishing snapshots so a rejected staging operation does not leave an invalid snapshot object.

Filesystem locks coordinate cooperating Knot processes. The implementation rejects symlinks observed in paths but does not claim isolation against hostile same-user filesystem changes between checks and opens. Use local filesystems; network filesystem locking and power-loss durability are not guaranteed. Unix directory fsync and file fsync improve durability; Windows directory durability remains dependent on its filesystem APIs.

## Scope and tradeoffs

M1 supports portable regular files and stable path casing. Case-only path transitions require an intermediate name and snapshot; this avoids divergent behavior on case-insensitive hosts. No automatic line-ending conversion, semantic format interpretation, binary merging, garbage collection, authentication, or external test execution occurs.

Blobs stream in 64 KiB buffers. Buffered object reads and structured objects are limited to 16 MiB. Text diff generation is limited to 1 MiB per side; larger assets receive hash/size summaries. Whole-file deduplication is implemented; block deduplication and compression are deferred.

A copied `.knot/` contains everything needed to reconstruct tracked bytes with `restore .`. Author configuration and issue history are copied too. Untracked files are outside the reproducibility guarantee.

## Acceptance progression

M1 verifies storage, identity, staging, history, branches/tags, checkout, recovery, JSON interfaces, and independent issues. The electromechanical test fixture uses CAD/PCB-like binary bytes plus firmware source; it does not claim geometric interpretation.

M2 adds stable-ID requirements and contract-aware impact. M3 resolves compatible configurations with previews, structured conflicts, ancestry ranges, and a documented bounded policy. M4 binds manual review/test evidence and traceability to exact inputs, gates releases, and audits operator overrides. M5 provides the Rust MCP server and five skills. M6 exchanges verified objects/refs and reconstructs equivalent working states. Integration tests cover these workflows without claiming geometric interpretation, automatic test execution, or a tested live internet deployment.

## M2 decisions

Engineering metadata belongs to the owner Revision. A dependency or export change therefore creates a metadata revision even when bytes are identical; it does not modify an existing revision. Repeated staging retains committed ancestry, and `add` preserves staged declarations. Snapshot/Commit encodings and all M1 golden hashes stay unchanged. Revision schema 2 and a conservative repository capability marker provide explicit forward incompatibility for old readers.

M2 checks exact declared revision hashes and exact interface versions. M3 adds ancestry-range solving; inferred physical compatibility remains deferred. Interface definitions are explicit opaque bytes, so unchanged contracts can survive a content revision; they are user-maintained declarations, not validation evidence. Default commit/checkout reject unsatisfied declarations, while explicit `--allow-incompatible` records/restores WIP without weakening integrity or dirty-file protections.

Graph queries default to staging and accept committed historical views. Impact without a baseline reports potential reverse reachability; `--since` compares actual selected revisions and interface exports, suppressing unchanged direct contracts. Downstream propagation is conservatively reported for review. These M2 queries remain distinct from M3 resolution and M4 manual evidence applicability; they do not execute engineering tests.

## Verification direction

Prioritize manual review records and dependency-change warnings before automated engineering validators. Review applicability is derived from exact recorded inputs, while downstream warnings identify unresolved upstream reviews. Matching declarations and human verification are separate states. No agent or extractor is required, and unchanged interface declarations alone must not suppress manual review of changed endpoint revisions. Evidence stays outside engineering state hashes. See [manual verification](verification.md). The implemented M4 report is separate from declared requirement status.

## M3–M6 decisions

- Resolution orders current dependency roots and paths, then prefers revisions by
  ancestor count and hash. It retains relationship keys, respects pins, and bounds
  search. Applying a result stages it; it cannot silently advance HEAD.
- Test and trace manifests are tracked files pointing to typed immutable objects.
  Procedures/definitions follow engineering commits. Evidence heads are independent
  refs, so a checkout finds applicable historical records without rewinding review
  discussion. Failed records for the same inputs supersede earlier passes.
- Dependency reviews bind exact endpoint revisions even when declared contracts
  match. Upstream warnings identify unresolved relationships instead of creating
  false local test failures. Automated validators can later emit the same evidence.
- The explicit `rape` operator command rebinds accepted requirements and records
  overrides. A commit using applicable override evidence stores its IDs and is
  displayed `(raped)`. This changes only new commit schema-2 objects; old hashes
  remain unchanged. Records distinguish override acceptance from test execution.
- Releases bind historical evidence and a snapshot policy; integrity checks verify
  that evidence directly rather than consulting mutable current review heads.
- MCP uses bounded stdio JSON-RPC without an async framework. Writes require local
  startup authorization, explicit apply, and expected HEAD/staging/worktree/reference state.
  Self-reported client names are never authorization.
- Synchronization transfers missing objects with streaming verification, then uses
  one recoverable ref/checkout transaction. Fast-forward-only rules prevent silent
  overwrites; issue/review forks are conflicts too. Bare peers avoid working-tree
  ambiguity. Ref names are checked as a portable, collision-free set.
- OpenSSH delegates account/key authentication and strict host verification to the
  OS. An optional Tailscale adapter supplies WireGuard/NAT traversal/relay support;
  ordinary core operations have no provider dependency. LAN multicast is opt-in
  discovery only. A discovered address is not proof of identity or permission.
- No GitHub-style host or React UI is planned. Block deduplication, partial clone,
  automated merge, background sync scheduling, and native VPN implementation remain
  separate future work.
