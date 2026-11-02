# Knot repository formats: M1–M6

All format rules are shared across adapters. Unsupported repository versions, object types, or object schema versions are errors; there is no implicit conversion.

## Layout

```text
.knot/
  HEAD                      # symbolic branch or detached commit pointer
  config.toml               # format_version=1, 2, or 3; hash="sha256", author, optional bare
  index                     # immutable staging Snapshot object pointer
  objects/sha256/aa/bb...    # complete immutable envelope, full digest split 2+62
  refs/heads/<name>          # commit pointer
  refs/tags/<name>           # commit pointer; immutable through M1 commands
  refs/issues/<uuid>         # latest IssueEvent pointer
  refs/reviews/<subject-id>   # latest ValidationRecord pointer
  refs/releases/<name>        # immutable Release pointer
  refs/remotes/<peer>/<branch> # fetched commit pointer
  remotes/<peer>              # local URL configuration, not exchanged
  transaction.json          # present only during a pending transaction
  logs/<transaction-uuid>.json
  locks/repository          # OS lock handle, not a PID or stale-lock marker
  cache/                    # disposable; currently unused
```

Initialization temporarily uses root `.knot-init-*` directories; these are reserved metadata and excluded from worktree scans. If initialization is killed before publication, an abandoned staging directory can be removed while no initializer is running.

Object IDs are exactly 64 lowercase hexadecimal characters. Pointer files contain the ID followed by exactly one LF. Symbolic HEAD contains `ref: refs/heads/<name>` followed by LF. An unborn branch has no branch ref yet. Branch/tag names are validated portable paths; HEAD, dot-prefixed components, `.lock` suffixes, `..`, `@`, and 64-character names are rejected. Command-created branch/tag names must not collide by case folding or directory prefix.

## Envelope and hashing

Every object file is the following bytes, with no extra newline:

```text
knot NUL type NUL schema_version_decimal NUL payload_length_decimal NUL payload
```

`payload_length_decimal` is the byte length, with no leading zeros except `0`. The object ID is SHA-256 over this entire envelope. Types are `blob`, `asset`, `revision`, `snapshot`, `commit`, `issue_event`, `dependency`, `interface`, `test_definition`, `validation_record`, `trace_link`, and `release`. Types use schema 1 except engineering revisions (schema 2), ancestry-range dependencies (schema 2), and commits carrying override evidence (schema 2). Schema-1 revisions have no engineering metadata; schema-2 revisions have at least one dependency or interface export. Readers check magic, schema version, canonical length, actual length, type, and digest.

A Blob payload contains arbitrary bytes. Other payloads are UTF-8 [RFC 8785 canonical JSON](https://www.rfc-editor.org/rfc/rfc8785.html), limited to 16 MiB. Typed decoding rejects unknown or duplicate fields; readers re-encode the value and require identical canonical bytes. IDs and UTC Unix-second timestamps are strings. Schemas do not use floating-point values. Canonical JSON key ordering uses UTF-16 ordering; snapshot entry ordering separately uses UTF-8 path order.

Golden vectors:

| Type | Payload | Object ID |
|---|---|---|
| blob | `hello` followed by LF | `ba0990d3338beef2f68c2f9329ea73f255984e23d32aa8489a2b8ceedc45c5bb` |
| snapshot | `{"entries":[]}` | `d6ee12f36aea8615b08be863465ef2955755a3a6d4aecfc693f788670fd1d22b` |

## Schemas

Fields below are required; nullable fields are explicitly `null`, not omitted.

- **Asset:** `{identity: UUIDv4}`. The object's ID is the stable asset reference. No filename is included.
- **Revision schema 1:** `{asset: object_id, blob: object_id, parents: [revision_id], executable: boolean}`. Every parent belongs to the same asset. Initial revisions have no parents. M1 creates zero or one parent; vectors allow later merge work without inventing merge behavior now.
- **Revision schema 2:** extends schema 1 with `dependencies: [dependency_id]` and/or `interfaces: [interface_id]`. Empty lists are omitted. Present lists are sorted by object ID with no duplicates. Dependencies must belong to the revision's asset; exported interfaces must belong to that asset and have distinct names. Parents can reference either revision schema.
- **Interface schema 1:** `{asset: asset_id, name: string, definition: blob_id}`. The stable logical interface identity is `(asset, name)`; its version is the Interface object ID. The definition is captured bytes, not extracted geometry or an execution result.
- **Dependency schema 1:** `{dependent: asset_id, target: asset_id, requirement: requirement_object}`. Requirements are `{kind: "revision", revision: revision_id}` or `{kind: "interface", name: string, interface: interface_id}`. Pins must refer to the specified target asset/name. Self-dependencies are rejected. A revision has at most one revision pin per target and one interface pin per target/name. Directed cycles across assets are allowed; traversal is cycle-safe.
- **Snapshot:** `{entries: [{path: string, revision: object_id}]}`. Entries are sorted, path-unique, and asset-unique. Paths cannot collide through case folding, directory components, or file/directory prefixes.
- **Commit schema 1:** `{snapshot: object_id, parents: [commit_id], message: string, author: string|null, timestamp: decimal_string}`. Message must be nonblank. Commit and revision ancestry are distinct.
- **IssueEvent:** `{issue: UUIDv4, parent: event_id|null, action: action_object, author: string|null, timestamp: decimal_string}`. Parent events belong to the same issue. The root action is exactly one create event.

Issue actions use a `kind` discriminator:

```text
create   {kind, title, body}
edit     {kind, title: string|null, body: string|null}
comment  {kind, body}
close    {kind}
reopen   {kind}
link     {kind, target: {kind: "commit"|"asset", id: object_id}}
```

Issue projection replays the event chain from oldest to newest. Referenced commits and assets must exist. The UUID identifies the issue across events; the event object ID is the optimistic concurrency token. There are no global numeric issue counters.

## State boundaries and recovery

The staging pointer addresses a complete immutable snapshot. `add` creates revisions parented to the committed asset revision, reusing the committed revision when bytes/mode are unchanged. Repeated staging does not introduce intermediate staged ancestors into committed history. Snapshots never include issue refs, commit timestamps, validation evidence, local configuration, or cache contents.

Transactions record version, UUID, purpose, ordered file changes, and ordered metadata changes. Each file change stores root-relative path plus nullable before/after blob-and-executable states. Metadata changes store their names and nullable before/after strings. Before/after blobs are published before the journal. The audit record contains the transaction and its completed/aborted outcome.

The journal is authoritative until removed. Recovery verifies recorded objects and expected file/ref states before replacement. Temporary `.tmp*` publication files in metadata/object directories are not objects or refs and are ignored during enumeration; they can be removed while no operation is running. Recovery and logs are not caches. M1 performs no garbage collection, so detached, staged, and abandoned immutable objects remain available.

## M2 capability upgrade and compatibility

New repositories begin with format marker 1. Before the first Dependency, Interface, or revision-schema-2 publication, the filesystem adapter atomically changes `config.toml` to `format_version = 2`, preserving author configuration. The marker declares the minimum reader capability; it is not engineering snapshot content. A failed operation or an aborted transaction may leave marker 2 and valid orphan objects, but cannot leave a published M2 object behind marker 1. Older M1 readers refuse marker 2.

No existing object, commit, snapshot, revision, asset ID, or hash is rewritten. For M2, Snapshot and Commit schemas stay at 1 because their existing references can reach either revision schema. M4 adds an opt-in commit schema 2 for override evidence, described below. Reading an older snapshot yields its original engineering graph (empty for M1). Removing all metadata from a newly staged revision allows the schema-1 encoding again, but the repository capability marker stays 2 because M2 history/objects remain stored.

Object serialization, hashing, and buffered reads check each type's supported schema versions. A schema-1 envelope containing engineering fields, or a schema-2 revision with all metadata omitted, is rejected. `fsck` verifies structural ownership, contract blobs, and exact reference types; it does not reject an explicitly recorded, structurally valid snapshot whose declared requirements are unsatisfied.

## M3–M6 capability marker and schemas

Before publishing range dependencies, commit override lists, or the new M4
object kinds, the adapter atomically raises `format_version` to 3. Bare peers also
use marker 3 and `bare = true`. Marker upgrades are monotonic; a checkout or abort
never downgrades the required reader. Old objects are never rewritten. Snapshot
schema 1 and all existing M1/M2 hash vectors remain unchanged.

- **Dependency schema 2:** same owner/target fields, with requirement
  `{kind: "revision_range", minimum: revision_id, maximum: revision_id|null}`.
  Both bounds belong to the target, and maximum, when present, descends from
  minimum. Inclusive ancestry determines membership. Revision-range and exact
  pins share a target requirement key; named interface requirements remain
  independent. Other requirement variants retain dependency schema 1.
- **Commit schema 2:** schema-1 fields plus a nonempty, sorted, unique `overrides`
  array of ValidationRecord IDs. Each record is an explicit dependency override
  whose exact input revisions/requirements apply to the commit snapshot. The
  original override snapshot may differ through unrelated changes. Empty lists
  are omitted and select schema 1. Human history derives `(raped)` from this list.
- **TestDefinition schema 1:** `{name, procedure: asset_id, inputs: [asset_id],
  description}`. Inputs are nonempty, sorted, and unique. The procedure is a
  stable asset identity; evidence selects its exact revision.
- **TraceLink schema 1:** `{source: asset_id, source_revision: revision_id,
  target: asset_id, target_revision: revision_id, relation}`. Both revision owners
  must match. Relation is `documents`, `describes`, `satisfies`, or `verifies`.
  Self-links are rejected.
- **ValidationRecord schema 1:** `{subject, parent: record_id|null,
  inputs: {asset_id: revision_id}, requirements: [dependency_id], outcome,
  reviewer, started, finished, environment, notes, artifacts: [blob_id]}`.
  Optional `override_snapshot: snapshot_id` is omitted for genuine review/test
  records. Subjects use a `kind` discriminator: `asset` with `asset`, `dependency`
  with `dependent` and `target`, or `test` with definition-manifest `definition`
  asset ID. Outcomes are `passed`, `failed`, or `error`. Times are decimal UTC
  Unix seconds with start <= finish. Reviewer, notes, environment, and inputs are
  nonempty. Requirements are sorted and unique; dependency evidence includes all
  requirements between the endpoint revisions. Test evidence includes definition,
  procedure, and every declared input. All asset/revision ownership is verified.
  Only passed dependency overrides may bind an override snapshot, and their
  exact inputs must match it. History parents retain the same subject.
- **Release schema 1:** `{name, commit: commit_id, snapshot: snapshot_id, policy,
  evidence: [record_id], approver, timestamp, notes}`. Policy fields are
  `require_dependency_reviews`, `require_all_tests`, `required_tests`, and
  `required_assets`; defaults are true/true/empty/empty. The selected tracked
  `knot-policy.json` supplies policy. The release's own historical evidence must
  cover all required subjects and exact inputs with passed outcomes. Later
  failures do not rewrite the historical release. Approver is nonblank text,
  not an authenticated signature.

Test and trace manifests are canonical Blob documents `{kind, object}` pointing
to TestDefinition or TraceLink objects. They are tracked as ordinary assets at
`tests/<name>.knot-test.json` and `docs/trace/<identity>.knot-link.json`. The `.knot-test.json` and `.knot-link.json` suffixes identify structured Knot
manifests; malformed descriptors are rejected before staging advances. They
therefore follow staging, commits, branches, and checkout without embedding
validation evidence in snapshots. Integrity checks resolve these pointers too.

Review refs use the SHA-256 object-domain hash of the canonical ReviewSubject as
an immutable subject identity; ref contents select the newest evidence record.
This identity is not itself a stored ValidationRecord. Evidence applicability
walks newest-first history, selecting the newest record with identical scoped
inputs and requirements. No matching record yields unverified/stale state.

## Synchronization protocol

Protocol version 1 uses bounded newline-delimited JSON control messages followed
by exact-length raw object frames. `advertise` returns committed refs, object IDs,
and the default branch. `get` names requested IDs and returns ordered `{id,len}`
frames. `put` names frames and expected-before/after ref updates; a writable
endpoint acknowledges readiness before accepting raw bytes, then responds after
verified, journaled publication. An error ends the session because a partial raw
frame cannot be safely reframed. Locks cover the session. Control records are
limited to 16 MiB; blobs stream in 64 KiB buffers.

Transient object files and refs are not mirrored between peers. Missing objects
are verified before publication, and the complete store/ref graph is verified
before synchronized refs advance. Ref updates are compare-and-swap and
fast-forward-only, with immutable tags/releases. Bare HEAD branches advance their
index without materializing files; working peers require a guarded checkout.
Local `remotes/<name>` JSON contains only `{url}` and remains outside transferred
history. OpenSSH or optional Tailscale SSH carries the same byte protocol.
