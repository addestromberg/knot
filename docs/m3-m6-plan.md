# M3–M6 implementation plan

Status: implemented in version 0.6.0, within the agreed manual-verification scope.

Keep the existing synchronous Rust core and recoverable filesystem backend. Preserve
M1/M2 canonical bytes. New object kinds/constraint schemas raise the repository
reader capability marker; old snapshots remain readable by the new implementation.

## M3: deterministic resolution

- Gather revisions from staged state and reachable commit/revision history.
- Solve exact revisions, named contracts, and ancestry-bounded revision ranges.
- Prefer descendants by ancestor count, then descending object ID, in stable asset
  order. Respect explicit pins; report conflicting requirements and bounded-search
  exhaustion separately.
- Preview exact selected revisions and file changes without writing repository
  state. Explicit application uses existing dirty-file, collision, locking, and
  recovery rules, stages the resolved snapshot, and leaves HEAD unchanged.

## M4: manual verification and traceability

- Store immutable exact-input review and validation evidence independently of
  engineering snapshots; preserve superseded results.
- Derive current/stale/unverified/failed states and upstream warnings. Reuse
  applicable evidence across checkout; unchanged declarations cannot hide changed
  endpoint revisions from manual review.
- Version test definitions/procedures as ordinary assets with declared input
  relationships. Record manual results, environments, notes, and artifact objects.
  Automatic CAD interpretation and external test execution remain optional later
  extensions, as agreed.
- Add versioned documentation/requirement relationships and named releases with
  explicit review/test requirements and validation gates.

## M5: agent interfaces

- Add a dedicated Rust stdio MCP binary using the same core operations.
- Support modern per-request MCP metadata plus the previous handshake protocol.
- Default to read-only access. Enable writes at server startup and require an
  explicit apply argument and expected state for every mutation.
- Provide the five repository/dependency/revision/validation/release skills.

## M6: synchronization

- Exchange immutable verified objects and refs over filesystem and SSH transports.
  SSH delegates authentication and host verification to the user's SSH setup.
- Transfer missing objects incrementally and stream large blobs.
- Verify complete object graphs before recoverable atomic ref publication. Use
  compare-and-swap expectations and refuse divergent updates.
- Support clone, named remotes, fetch, push, and fast-forward-only pull. Preserve
  independent issues, reviews, and release refs. Never overwrite a dirty checkout.

## Acceptance and validation

Extend behavior tests for deterministic selection/conflicts, stale manual reviews,
traceability, release gates, MCP authorization/preview/state changes, two-computer
synchronization, large objects, corruption, concurrency, and interrupted updates.
Run locked offline tests, rustfmt, Clippy, and cross-platform compile checks.
Update schemas, CLI examples, architecture decisions, and milestone status at each
completed stage. The product is CLI-only; the previously proposed hosting/React milestone has been removed. Optional Tailscale integration provides coordination and relay fallback for automatic WireGuard connectivity; LAN discovery remains opt-in.

## Verification record

All 94 automated tests pass on Linux, including a two-repository 18 MiB transfer,
manual evidence/release restoration, override history, MCP write authorization,
stale-preview rejection, SSH endpoint uploads/downloads, ref conflicts, and crash
recovery. Rustfmt, Clippy with warnings denied, and the locked workspace build
pass. All-targets compile checks pass for Windows GNU and macOS ARM; native
runtime coverage for those systems belongs to the CI matrix. Direct dependency
versions were rechecked against crates.io; no newer stable versions or compatible
lockfile updates were found. Skills passed the bundled skill validator.

Regressions for GitHub issues #1–#5 cover interrupted uploads/downloads at every
complete frame boundary, killed receivers and retry, incomplete object graphs,
release/branch selector collisions, MCP branch/detached-HEAD changes, portable ref
directory spellings, and edit/revert restoration of revisions and test evidence.

Live LAN multicast, internet SSH, and Tailscale account/VPN connectivity were not
exercised against external devices. Those require real network access and
operator-authorized peers. No VPN daemon was installed or reconfigured during
implementation. Automated engineering test execution remains an optional future
extension; records in this version describe manual measurements/assessment.
