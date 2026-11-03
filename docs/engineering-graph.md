# M2 engineering graph

M2 adds immutable dependency/interface declarations and snapshot-based graph queries. M3 now adds compatible revision resolution; M4 separately records manual verification. This guide retains the M2 declaration model.

## What follows a commit

A Snapshot references exact asset Revisions. An engineering Revision references its content Blob, exported Interface objects, and outgoing Dependency objects. Dependency declarations therefore follow staging, commits, branches, and checkout. A metadata-only edit creates a new Revision while reusing the same Blob. Renames reuse the Revision and stable Asset identity.

```text
Commit → Snapshot → Revision → Blob
                       ├── Interfaces → definition Blobs
                       └── Dependencies → stable target Assets
                                            └── exact Revision or Interface version
```

No dependency definition is kept in an unversioned global registry. Object references provide the authoritative graph; in-memory maps are derived query data.

The working directory materializes one selected revision per tracked asset at the snapshot's recorded path. Older versions remain immutable objects inside `.knot/`; checkout does not add historical copies beside the working files. A snapshot rejects duplicate asset identities. Explicit copies get separate identities.

## Exact revision and interface requirements

An exact revision requirement selects an asset identity plus one Revision object ID. A different revision of that asset does not satisfy the pin, even if it retains an identical interface. Names such as R5 are explanatory engineering labels, not current CLI revision syntax.

An interface requirement selects target asset identity, case-sensitive interface name, and one Interface object ID. The target's selected Revision must export that exact interface version. The version depends on owner identity, name, and captured definition bytes. Changing CAD bytes while retaining the declared definition preserves an interface requirement. Changing/removing the definition, or removing the target asset, makes it unsatisfied.

Interfaces are explicit user-maintained contracts. Knot does not inspect CAD geometry, KiCad connectivity, or prove physical compatibility. `declarations_only: true` makes that boundary explicit in graph/dependency/impact reports. Matching declarations are not test evidence.

Self-dependencies are rejected. Multiple assets can form directed cycles, particularly through interfaces; queries use visited sets and terminate. A cycle of incompatible exact revision pins is reported through ordinary unsatisfied edges rather than being silently relaxed.

## Commands and selection

```sh
knot depend add pcb.kicad_pcb --on housing.FCStd
knot depend add firmware.rs --on pcb.kicad_pcb
knot deps pcb.kicad_pcb
knot deps housing.FCStd --reverse --transitive
knot graph --json
knot commit -m "Record exact engineering requirements"

knot interface set housing.FCStd --name pcb_mount --file docs/pcb_mount.json
knot interface list housing.FCStd
knot depend remove pcb.kicad_pcb --on housing.FCStd
knot depend add pcb.kicad_pcb --on housing.FCStd --interface pcb_mount
knot commit -m "Use declared mounting contract"
```

Queries default to the staged snapshot. `--at <ref|commit>` selects history; `HEAD` means committed state, not unstaged edits. Asset selectors accept paths relative to the operation directory or full stable Asset object IDs. `deps` and `graph` expose both asset and revision IDs. An explicit `depend add --revision <hash>` must pin a revision belonging to the named target; its selected revision may remain different until the project configuration is adjusted.

`interface set` captures the current regular file's exact bytes immediately. It does not implicitly stage that file or infer changes to the engineering asset. Track and stage the definition file separately if you want it restored by pathname. Re-register after changing its contents. The captured definition Blob is retained by the Interface object even when the source file is untracked.

There is at most one revision pin per source/target and one interface pin per source/target/name. Adding the same key replaces its requirement. Revision pins and different named interface requirements coexist and must all be satisfied. Removing with only `--on` removes all requirements for that target; adding `--interface` restricts removal to that name. Removing a selected target does not erase its dependents; use the stable ID from `deps` to remove a dangling requirement after its path has disappeared from history.

## Working files, staging, and HEAD

Editing a file does not advance HEAD. `add` creates immutable staged revisions and updates the staging snapshot pointer; a successful `commit` advances the current branch's commit ref (or detached HEAD). Failed dependency checks leave both HEAD and your staged work intact.

`checkout main` selects the branch tip's complete recorded snapshot. It does not independently select the newest revision of each asset. An older pin is not obsolete merely because a newer revision exists in another branch or in history; it becomes unsatisfied when the selected target no longer matches its requirement.

| Action | Working files | Staged snapshot | HEAD |
|---|---|---|---|
| Edit | Changed | Unchanged | Unchanged |
| `add <path>` | Retained | New exact revisions | Unchanged |
| `depend add` / `interface set` | Retained | New metadata revision | Unchanged |
| Successful commit | Retained | Retained | Advances |
| Rejected commit | Retained | Retained | Unchanged |

Dependency checks inspect staging, not unstaged file bytes. After editing a model, use `add`, `status`, `impact --since HEAD`, and `graph` before committing. If a PCB's exact pin breaks, adapt or independently verify the PCB, then explicitly update its requirement. Do not repin merely to silence a conflict. Exact firmware/test-procedure consumers may also need updated requirements when their target PCB revision changes.

For an interface-based dependency, a CAD change retaining the same declared interface remains satisfied. You must explicitly update the contract definition and re-register it when the physical interface changes. Knot cannot detect undeclared geometric or electrical changes or establish physical pass/fail evidence automatically. M4 records explicit manual verification and staleness.

## Status, conflicts, and explicit WIP

Graph statuses are:

| Status | Meaning |
|---|---|
| `satisfied` | The selected revision/export matches the declared exact requirement |
| `missing_asset` | The target exists historically but is absent from the selected snapshot |
| `revision_mismatch` | The selected target revision differs from its exact pin |
| `missing_interface` | The selected target has no export with the required name |
| `interface_mismatch` | The name exists but exports another version |

`status` reports `requirements_satisfied` and `dependency_conflicts`; staged diffs identify `constraints` and `interfaces` changes even when bytes are identical. Staging is permitted to be incomplete. Default commit and checkout reject unsatisfied requirements. JSON errors use exit 3, `dependency_conflict`, and `details.conflicts` with source, target, expected requirement, actual selection, and status.

Human-readable `status` summarizes file/change counts, satisfied requirements and
conflicts, and verification states. Dependency conflicts and failed/error checks
remain individually visible; unverified/stale checks and upstream review needs
are counted. `status --verbose` lists staged/unstaged/untracked paths, the complete
staged dependency tree, and all verification entries. Parent nodes depend on child
nodes; each asset is expanded once, with numbered references for shared
dependencies and cycles. Nodes show the selected revision and change markers.
Edges show the required revision or named interface and whether it matches,
including missing targets/exports and mismatches. Disconnected files are included
in the verbose tree. Unstaged edits produce an explicit warning and do not
silently change the graph. `graph` always uses the full renderer for staged or
historical views. JSON status includes the full report as `dependency_graph`
regardless of `--verbose`, while preserving existing fields; status also includes
a separate `verification` report for M4 evidence. Graph matches alone do not
establish engineering verification.

`commit --allow-incompatible` explicitly records a WIP snapshot; `checkout --allow-incompatible` explicitly restores one. These overrides do not bypass content integrity, object ownership, recovery, or dirty-file protection. `fsck` checks structural integrity and can pass for an intentionally recorded incompatible configuration.

## Impact and the housing/PCB/test example

`impact housing.FCStd` lists potential direct and transitive dependents. `impact housing.FCStd --since HEAD` compares selected state against a committed baseline. Root changes distinguish content, interfaces, constraints, path, addition/removal, and otherwise changed revision identity. A rename alone does not change dependency requirements.

For comparisons, direct exact-revision consumers are affected when the target revision changes. Direct interface consumers are affected when the required named export changes, not merely when bytes change. Unchanged contracts appear under `unaffected_direct`; those paths are excluded from propagation. Once a consumer is affected, further reverse reachability is conservatively reported as transitive review candidates. This is impact analysis, not evidence invalidation or a guarantee that every downstream behavior changed.

For PCB R3 pinned to Housing R5, with firmware/electrical procedures depending on PCB R3 and a mechanical procedure depending on Housing R5, staging Housing R6 reports the PCB and mechanical procedure directly, and the firmware/electrical procedures transitively. Exact pin mismatches prevent an ordinary commit until requirements/configuration are updated or WIP is explicitly allowed. Checkout restores the historical graph as well as file bytes.

Test procedures and documentation are ordinary assets. M3 implements compatible selection and ancestry ranges; M4 implements manual evidence, versioned test manifests, traceability links, and release gates. Automated engineering runners remain optional future extensions.

## Manual verification workflow (M4)

A changed pipe revision makes its rod-fit review stale even when its declared
interface stays unchanged. Status reports that relationship separately from
satisfied requirement pins, and identifies downstream assets as unverified
upstream. A person can confirm that an external mount leaves fit unchanged and
record the exact revision pair with notes. This does not change the rod's bytes.

Records remain immutable and independent of snapshots/HEAD. Historical checkout
can reuse applicable evidence; unrelated file edits and path-only renames do not
invalidate it. Release policies require current evidence. Explicit operator
acceptance through `rape` is audited and marks commits `(raped)`.

See [manual verification, tests, and releases](verification.md) and
[compatible resolution](resolution.md) for commands and exact applicability rules.

## Format and verification

M2 adds Interface/Dependency schema 1 and Revision schema 2. Snapshot and Commit encodings and legacy hashes stay unchanged. The first M2 object publication conservatively upgrades the repository reader-capability marker; old binaries refuse the new marker. See [format.md](format.md).

Tests cover exact/interface constraints, transitive/reverse queries, cycles, renamed/deleted assets, metadata-only changes, historical restoration, deterministic hashes, foreign references, explicit WIP, and interrupted dependency staging. Run:

```sh
cargo test -p knot-store --test dependencies --locked
cargo test -p knot-store --test recovery --locked
cargo test -p knot-cli --test cli --locked
```
