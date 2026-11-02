# Knot product specification and roadmap

This document records the product direction and implemented M1–M6 boundaries. Knot is a Rust CLI application with an MCP adapter for agents. The previously proposed hosted platform and React frontend have been removed from the roadmap.

## Vision and principles

Knot is a universal, Git-like engineering repository whose state is a versioned graph of interdependent assets. Projects may contain FreeCAD/STEP/STL models, KiCad schematics/PCBs, firmware, applications, configuration, documentation, tests, simulations, manufacturing outputs, and their results.

Core operations work offline without an external database or mandatory hosted service. `.knot/` stores content-addressed immutable history and filesystem metadata. SHA-256 is the initial hash algorithm. Serialization is canonical and schema-versioned; type and schema participate in hashing. Identical repository inputs produce deterministic states, and the same repository data can reconstruct exact snapshots on another machine.

Arbitrary text and binary contents are supported within explicit portable filesystem rules. Stable asset identities are independent of filenames. Dependency relationships, compatibility, validation evidence, releases, and engineering decisions are first-class domain concerns. Derived caches/indexes must be rebuildable. Large-file chunking and block deduplication are future work.

Immutable history, verified objects, atomic pointer updates, locking, guarded checkout, and crash recovery take precedence over feature count. Binary conflicts must never be silently merged or overwritten. Semantic extractors, validators, and dependency resolvers will be extensible plugins. All interfaces share domain logic; agent workflows cannot bypass integrity rules.

## Target object model

| Object | Responsibility |
|---|---|
| Blob | Exact immutable bytes |
| Asset | Stable logical identity |
| Revision | Immutable asset content, metadata, and relationships |
| Interface | Versioned technical contract |
| Dependency | Relationship and compatibility constraints |
| Snapshot | Complete resolved set of exact asset revisions |
| Commit | Snapshot plus historical parentage and provenance |
| Test Definition | Versioned test specification |
| Validation Record | Evidence tied to exact inputs and test versions |
| Release | Named approved snapshot and applicable policy |
| Issue event | Independently versioned repository-local engineering discussion |

M1 implements Blob, Asset, Revision, Snapshot, Commit, and IssueEvent. M2 adds Dependency and Interface objects plus engineering revision schema 2. M3 adds ancestry ranges; M4 adds TestDefinition, ValidationRecord, TraceLink, Release, and commit-linked operator overrides. Current wire schemas and storage rules are specified in [format.md](format.md).

Content state, dependency state, and validation state must remain distinct. Evidence references the state it validates and cannot be included in that state's hash. Documentation is a normal versioned asset, with explicit traceability relationships added later. Issue activity does not change engineering snapshot hashes or rewind on checkout.

## Versioning and compatible states

The CLI provides familiar initialization, staging, status, commits/history, diffs, branches, tags, checkout, and restore. Branches refer to commits; snapshots identify exact configurations. Explicit renames preserve identity. Local modifications and untracked checkout destinations are protected. Exact branch/tag checkout reconstructs the recorded revisions without compatibility resolution.

M2 dependencies attach to stable asset IDs, support asset-to-asset exact revision pins and asset-to-interface exact contract versions, and provide transitive/reverse queries and impact analysis. M3 supports inclusive ancestry ranges and deterministic compatible revision selection. M4 records applicable manual evidence and release policies. Hardware/software extractors remain optional future work.

The graph must distinguish file-byte changes, interface changes, changed constraints, and required revalidation. A modified enclosure does not automatically invalidate a PCB if its required mounting contract remains compatible. Arbitrary binaries cannot prove geometric/electrical compatibility; unsupported checks return unknown.

`checkout <asset>@<full-revision-hash> --resolve` previews a compatible configuration before mutation; `--apply` explicitly stages its checkout without moving HEAD. Explicit pins are respected. A documented deterministic policy selects the newest compatible candidates, independently of validation. For Housing R10, PCB R6 requiring R9, PCB R7 supporting R10, Firmware R12 supporting PCB R7, and Firmware R13 requiring PCB R8, the expected result is Housing R10 / PCB R7 / Firmware R12. Unsatisfiable or unestablished compatibility returns structured conflicting requirements.

`checkout main --validated` requires applicable evidence under the target snapshot’s release policy, independently of constraint resolution.

## Documentation, validation, and releases

Requirements, interface specifications, decisions, procedures, manufacturing instructions, release notes, and reports are versioned assets. References identify exact revisions or explicit compatible ranges. The graph should answer which requirements a component satisfies, which documents describe its interfaces, which tests verify a requirement, and which documents may be stale.

M4 implements manual verification and dependency-change warnings. A review record identifies the dependency, exact endpoint revisions and requirement, reviewer, time, outcome, and notes or supporting artifacts. Changed inputs make the prior review stale for the selected configuration; direct and transitive dependents are flagged for review, not declared physically incompatible. Even unchanged interface declarations do not establish that modified geometry still conforms. Rechecking an unchanged snapshot may reuse its applicable review history.

Automated tests and semantic extractors are optional later extensions, not prerequisites for dependency tracking. Checks may include KiCad ERC/DRC, geometry/PCB fit, firmware builds, software tests, simulation, hardware-in-the-loop, measurements, and manufacturing verification. An automated execution additionally records its test definition/version, toolchain/environment identity, start/end times, pass/fail/error status, logs, measurements, and artifacts.

Status distinguishes passed, failed, not executed, stale, and unknown/unsupported. Input changes invalidate only evidence whose declared inputs changed. Releases can require a configurable mandatory set of validations and approvals. An issue comment or an author string is not approval evidence.

## Interfaces and agent workflows

CLI JSON output, structured errors, and stable exit codes are required. The core cannot depend on CLI, MCP, or UI presentation. Implemented MCP tools include repository info/status/history/assets, dependencies/impact, resolution/checkout, revision creation, validation execution/results, and release creation. State-changing tools need previews and explicit authorization boundaries.

The five skills under `skills/` cover repository, dependency, revision, validation, and release management, with later mechanical, electrical, firmware, software, simulation, and documentation workflows. A typical agent inspects state/dependencies, selects a revision, edits files, stages revisions, checks impact, executes required validations, links evidence to exact inputs, and commits or reports unresolved requirements. Skills define workflow; the core enforces integrity.

M1 adds basic local issues for this future use: create/list/show/edit/comment/close/reopen/link, Markdown bodies, JSON, immutable event history, and optimistic updates. Assignment, labels, priorities, and custom states are deferred.

## Peers and optional mesh connectivity

M6 implements decentralized clone/fetch/push/pull and remote refs over filesystem, SSH, and optional Tailscale SSH transports. Transfers exchange immutable objects and required history, verify integrity, detect missing data, update refs atomically, detect conflicts, and optionally authenticate. Large-file transfer, partial/shallow clones, and remote issue reconciliation will need their own policies. A remote is never required for local operations.

The CLI is the product interface; no web UI or required hosting service is planned. Opt-in LAN multicast supplies untrusted peer connection hints. An optional Tailscale adapter can configure WireGuard connectivity and delegate NAT traversal/relay fallback to a provider, including compatible self-hosted coordination. Plain local/SSH operations remain service-independent. Discovery never grants access.

## Milestones and acceptance

M1–M6 are implemented within the boundaries below. Optional automatic engineering validators remain deferred in favor of the agreed manual verification workflow. See the detailed [implementation plan](m3-m6-plan.md) and command guides.

| Milestone | Deliverable | Acceptance |
|---|---|---|
| M1 | Rust CLI/libraries; filesystem objects, staging, revisions, commits, hashes, branches/tags, guarded checkout/recovery; basic local issues | Copy repository data and reconstruct identical tracked bytes and snapshot hashes; retain issue history independently |
| M2 | Dependencies, interfaces, exact constraints, traversal/reverse queries, impact | Direct/transitive dependents are identified correctly |
| M3 | Compatible revision selection, deterministic resolution, conflicts, preview | Select a compatible CAD/PCB/firmware combination or explain failure/unknown compatibility |
| M4 | Manual review records, dependency-change warnings, staleness, documentation traceability, release policies; automated runners later | Changed inputs flag affected relationships; explicit reviews apply only to their recorded configuration |
| M5 | Rust MCP server, authorization boundaries, Knot skills | Agent can inspect, modify, validate, and commit through defined interfaces |
| M6 | Object/ref transfer and remote commands | Two machines exchange history and reproduce equivalent states without a database |

The primary eventual end-to-end scenario is:

1. Create Housing R1, PCB R1 compatible with Housing R1, and Firmware R1 compatible with PCB R1.
2. Commit snapshot S1, including the mechanical interface specification, PCB fit test, and firmware build test.
3. Create Housing R2 with changed mounting holes; report that PCB R1 may be incompatible.
4. Create compatible PCB R2 and Firmware R2; resolve snapshot S2.
5. Run affected validations and link exact evidence to S2.
6. Restore S1, then S2, checking hashes, bytes, compatible selections, and evidence bindings.

M1/M2 tests cover storage/history/reconstruction, exact pins, interfaces, and graph restoration. M3 tests compatible selection, ranges, conflicts, preview purity, and checkout protection. M4 tests exact-input review/test staleness, traceability, release gates, and audited overrides. M5 tests MCP protocol, authorization, stale-preview guards, and an agent write/commit workflow. M6 tests two-repository exchange, large objects, integrity failures, conflicts, endpoint framing, and ref-publication recovery. External engineering test runners and live internet/VPN deployment remain outside these tests.

Initial non-goals include hosting/web, live collaborative editing, automatic binary merging, full CAD interpretation, arbitrary dependency discovery, distributed builds, cloud synchronization, and complex user management. Prioritize correctness, determinism, recoverability, and the CLI.
