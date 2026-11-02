# Knot

Knot is an offline-first engineering version control system written in Rust. It stores immutable, SHA-256-addressed objects in `.knot/`, tracks stable asset identities, and maintains repository-local issues without a database or hosted service.

M1–M6 implement filesystem versioning, local issues, dependency graphs, deterministic compatible revision selection, manual verification, traceability, releases, MCP/skills, and filesystem/SSH peer synchronization. An optional Tailscale adapter supplies automatic mesh connectivity. The CLI is the application interface; no UI or hosted platform is planned. Geometry interpretation and automated engineering test runners remain optional future extensions.

## Build and check

The workspace pins stable Rust 1.99.1 in `rust-toolchain.toml`. Install Rust using [rustup](https://rustup.rs/), then:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p knot-cli -- --help
cargo install --path crates/knot-cli --locked
cargo install --path crates/knot-mcp --locked
```

Dependencies require an initial download; repository operations themselves never access the network. After `cargo fetch --locked`, builds can also use `--offline`.

## Version an engineering project

```sh
knot init groundstation --author "A Local Engineer"
cd groundstation
# Create mechanical, electronics, firmware, documentation, and test files.
knot add .
knot status
knot diff --staged
knot commit -m "Initial design"
knot tag v1.0
knot branch feature/new-housing
knot checkout feature/new-housing --preview
knot checkout feature/new-housing
knot mv mechanical/housing.FCStd mechanical/enclosure.FCStd
knot commit -m "Rename enclosure asset"
knot log mechanical/enclosure.FCStd
knot checkout v1.0
knot fsck
```

`branch` creates a branch without switching. Branch checkout attaches `HEAD`; tags and full commit IDs detach it. Tags cannot be overwritten. `log --target v1.0 path` can inspect a path present in an older state. Commits record the staged state, even when working files have further edits. Unchanged commits are rejected.

Working directories contain one selected revision per tracked asset at its normal pathname, such as `mechanical/pipe.step`. Historical revisions stay in `.knot/objects/`; Knot does not create `pipe_R1.step`, `pipe_R2.step`, or parallel revision directories. Checkout replaces the selected tracked contents and applies recorded renames/deletions, while protecting local modifications and untracked files. Intentional copies are separate assets.

`add <paths>` stages additions, modifications, and tracked deletions within those scopes. Paths are relative to the current directory or `-C <directory>`. Root `.knotignore` accepts Git-style patterns; `.git/` and `.knot/` are always excluded. Tracked files remain tracked when later ignored. There is no implicit use of `.gitignore`.

Use `knot mv` to preserve asset identity. External moves are deletion plus addition, and copies get new identities. Case-only path transitions require a distinct intermediate name and snapshot for consistent behavior across platforms.

`diff` compares working files with staging; `diff --staged` compares staging with `HEAD`. UTF-8 text receives unified diffs. Binary files, files containing NUL, and files larger than 1 MiB receive hash/size summaries.

## Dependencies and engineering interfaces (M2)

Dependency declarations are staged on immutable asset revisions. They follow commits, branches, and checkout. Queries use the staged snapshot by default; use `--at <branch|tag|commit>` to inspect history. Working edits must be staged before they affect graph queries.

`knot status` shows the branch/HEAD, staged and unstaged changes, untracked files, and a dependency tree. Parent entries depend on their children. Each tracked asset appears once with its selected revision; shared dependencies and cycles refer back to numbered entries. For example, after staging a pipe change:

```text
On branch main · HEAD 9b2716d4e508

Staged changes:
  modified pipe.step

Dependency graph (staged; parent depends on child)
  #2 rod.step @461df9c812e0
  └── #1 pipe.step @8f317b6d04ca [staged] — requires @57a49e2c108b [REVISION MISMATCH]

2 tracked files · 1 dependency · 1 conflict
WARNING: unsatisfied requirements block ordinary commits.
Matching requirements do not establish engineering verification.

Verification:
  [STALE] rod.step -> pipe.step
```

Revision IDs above are illustrative. `[matches]` means an exact requirement matches, not that an engineering check passed. Unstaged edits are marked and produce a warning because checks use staging. `knot graph` uses the same tree presentation; `graph --at <ref>` inspects history. `status --json` retains its versioned envelope and existing fields, and adds `data.dependency_graph` with the complete staged graph and full object IDs. `graph --json` retains its existing report.

```sh
# Pin the PCB to the model's exact staged revision.
knot depend add electronics/controller.kicad_pcb --on mechanical/housing.FCStd
knot deps electronics/controller.kicad_pcb
knot deps mechanical/housing.FCStd --reverse --transitive
knot graph --json
knot commit -m "Record PCB mounting requirement"

# Edit the housing, then stage it before inspecting the consequences.
knot add mechanical/housing.FCStd
knot impact mechanical/housing.FCStd --since HEAD --json
knot status --json
```

Exact pins refer to revision hashes, not filenames or sequential labels like R5. `deps` and `graph` expose stable asset IDs and revision hashes. `depend add --revision <hash>` pins an older existing revision of the same target asset; omitting it pins the target's staged revision. Asset selectors accept a tracked path or full stable asset ID. Use `mv` to preserve identity across renames.

A named interface allows a requirement to remain satisfied when the file bytes change but the declared contract remains identical:

```sh
knot interface set mechanical/housing.FCStd --name pcb_mount --file docs/pcb_mount.json
knot depend add electronics/controller.kicad_pcb --on mechanical/housing.FCStd --interface pcb_mount
knot interface list mechanical/housing.FCStd
knot add docs/pcb_mount.json
knot commit -m "Declare mounting interface"
```

The interface definition is an opaque, immutable blob captured from `--file` at registration time. Track that file separately if you want it restored at its pathname. Updating the file does not automatically update the exported interface; run `interface set` again. Interface versions depend on owner identity, name, and exact definition bytes. Names are case-sensitive and match `[A-Za-z_][A-Za-z0-9_.-]{0,63}`.

An exact revision pin and an interface pin are independent requirements. Adding an interface requirement does not remove an existing exact revision pin. When switching strategies, remove the existing requirements first:

```sh
knot depend remove electronics/controller.kicad_pcb --on mechanical/housing.FCStd
# Then add the desired exact or interface requirement.
```

`depend add` replaces an existing requirement for the same target and kind/name. `depend remove --on <asset>` removes all requirements on that target; `--interface <name>` removes only that named requirement. `interface remove <asset> --name <name>` stages removal of an export and may leave dependents unsatisfied.

`impact <asset>` lists potential direct and transitive dependents. `impact <asset> --since <ref>` compares staged state with a committed baseline, distinguishes content/interface/constraint/path changes, and excludes direct consumers of unchanged interface versions. Transitive entries are conservative review candidates. These queries describe declared-contract impact; status separately reports manual review applicability, even when an interface declaration stays unchanged.

Graph edge statuses are `satisfied`, `missing_asset`, `revision_mismatch`, `missing_interface`, and `interface_mismatch`. Reports set `declarations_only: true`: satisfying a declaration does not prove CAD fit or electrical compatibility. Unchanged exported interfaces are maintained explicitly by the user; Knot does not extract geometry or pin mappings.

Commits and exact checkouts reject unsatisfied declarations with exit 3 and JSON code `dependency_conflict`, including conflicting edges in `error.details.conflicts`. Staging can be incomplete while you work. To deliberately save/restore a work-in-progress configuration, supply `commit --allow-incompatible` or `checkout --allow-incompatible`. These flags do not bypass dirty-file protection or integrity checks.

Existing M1 repositories remain readable and their object hashes stay unchanged. The first M2 object write atomically raises the repository format marker to 2 before publishing engineering objects. Older M1 binaries then refuse to open it. This capability marker is not downgraded by checkout or abort; historical objects are never rewritten. See [engineering graph design](docs/engineering-graph.md).

## Resolve, verify, and release

```sh
knot resolve --pin mechanical/pipe.step@<full-revision-hash>
knot resolve --pin mechanical/pipe.step@<full-revision-hash> --apply
knot verify mechanical/rod.step --on mechanical/pipe.step --reviewer "Engineer" \
  --notes "Inspected bore and clearance"
knot validate --affected
knot commit -m "Record reviewed assembly"
knot release create v1.0 --approver "Engineer"
```

Resolution previews by default. Application stages a configuration and leaves HEAD
unchanged. Reviews bind exact inputs independently of snapshots; modified inputs
make prior evidence stale. Unresolved upstream reviews appear in status.

`knot rape` explicitly overrides dependency reviews with a guard for each
relationship; `--forced` accepts every guard. It rebinds accepted requirements,
keeps immutable override records, and labels commits using them `(raped)` in
`knot log`. This does not record execution of an engineering test. See the
[verification guide](docs/verification.md) for test definitions and release policy.

## Synchronize with peers

```sh
knot init /srv/knot/project --bare
knot remote add origin /srv/knot/project
knot push
knot clone /srv/knot/project ./copy
knot fetch
knot pull
```

Use `ssh://user@host/absolute/path` for authenticated SSH peers. Optional
`knot peer discover` finds LAN announcements. For automatic WireGuard-based
connectivity without port forwarding, install/authorize Tailscale and use:

```sh
knot peer mesh-up --ssh
knot peer discover --mesh
knot peer connect workshop --host workshop --user engineer \
  --repository /home/engineer/project
knot push workshop
```

Mesh coordination/relay services are optional and separate from core repository
operations. Plain local and reachable SSH peers need no such service. See the
[synchronization guide](docs/synchronization.md) for access policies and limits.

## Restore and recover

```sh
knot restore mechanical/enclosure.FCStd --preview
knot restore mechanical/enclosure.FCStd
knot restore --staged mechanical/enclosure.FCStd
knot restore --source v1.0 mechanical/housing.FCStd --preview
knot rm obsolete.step --preview
```

`restore` deliberately replaces selected tracked working files with their staged contents. `--staged` changes staging instead, using `HEAD` by default; `--source` selects another committed state. Preview output distinguishes staging changes from working-file changes. `rm` requires the file to have no staged or unstaged modifications.

Checkout requires clean staging and tracked working files. It refuses untracked destination collisions, including ignored files. Unrelated untracked files are retained. To reconstruct a working directory from a complete copy of `.knot/` made while no operation is running, run `knot restore .`, then `knot fsck`.

Mutations hold an exclusive OS lock. Interrupted multi-file operations leave `.knot/transaction.json` and block further mutations:

```sh
knot status --json
knot recover --continue
# Alternatively, before the transaction is finalized:
knot recover --abort
```

Recovery checks file fingerprints and refuses unexpected external edits. A completed audit log fixes the outcome; a crash after logging completion must be recovered with `--continue`. Checkout is recoverable rather than a single atomic directory-tree replacement.

## Local issues for humans and agents

```sh
knot issue create --title "Verify PCB fit" --body "Measure the mounting holes."
knot issue list
knot issue show <issue-id> --json
knot issue comment <issue-id> --body-file measurements.md
knot issue edit <issue-id> --title "Repeat PCB fit measurement"
knot issue link <issue-id> --asset mechanical/housing.FCStd
knot issue link <issue-id> --commit HEAD
knot issue close <issue-id> --expected-head <event-hash>
knot issue list --all
knot issue reopen <issue-id>
```

Issues use UUIDs; unique prefixes of at least four characters are accepted. They have Markdown descriptions/comments, open/closed status, and links to exact commits or stable assets. `show` includes the event head and history. Supply `--expected-head` on updates to reject stale edits. Author metadata is descriptive, not authentication or approval.

Issue history lives under its own refs and remains current across engineering checkouts. It does not change project snapshot hashes. Assignments, labels, priorities, and custom workflows are deferred.

## Automation and platform boundaries

Every operation supports `--json`, returning one envelope with `schema_version: 1`, `ok`, and either `data` or `error`. Errors contain stable `code`, `message`, and `details` fields. Machine output goes to stdout; human diagnostics go to stderr. Without `--json`, status/graph/history use readable terminal output; other results use pretty-printed JSON. Help uses conventional CLI text.

| Exit | Meaning |
|---|---|
| 0 | Success |
| 1 | Operational failure or missing resource |
| 2 | Invalid usage/input |
| 3 | Conflict, dirty working tree, or required recovery |
| 4 | Integrity or unsupported-format failure |
| 5 | Repository busy |

M1 supports portable regular files on Linux, macOS, and Windows. It preserves bytes and executable metadata, applying executable permissions on Unix. It rejects symbolic links, special files, non-UTF-8/non-NFC paths, reserved names, paths over 240 UTF-8 bytes, and case-folding collisions, including directory components. It does not preserve ownership, ACLs, arbitrary permission bits, modification times, or empty directories.

Use a local filesystem supporting locks and atomic file replacement. File contents are synced before publication; directory sync is implemented on Unix. Hardware power-loss guarantees depend on the platform and filesystem. Keep editors from modifying affected paths while an operation or recovery runs; repository locks coordinate Knot processes, not unrelated applications.

## Design and contribution

See [compatible resolution](docs/resolution.md), [manual verification and overrides](docs/verification.md), [MCP and skills](docs/mcp.md), [peer synchronization and mesh networking](docs/synchronization.md), [Repository Guidelines](AGENTS.md), [product specification](docs/specification.md), [architecture decisions](docs/architecture.md), [repository format](docs/format.md), and [dependency policy](docs/dependencies.md), and [engineering graph design](docs/engineering-graph.md).
