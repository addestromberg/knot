# Dependency policy

Verified on 2026-11-02 against crates.io's `max_stable_version` and Rust's stable release manifest. Rust 1.99.1 is the current stable toolchain and is pinned in `rust-toolchain.toml`. All direct dependencies selected for M1 use their latest stable release; there are no intentional older-version exceptions. Rechecked for M2 on the same date: no newer stable direct releases or compatible lockfile updates were found, and M2 adds no external dependencies. Rechecked again for M3–M6 on the same date: all direct stable versions still match, and no new external Rust dependencies were added. OpenSSH and Tailscale are optional runtime transport programs, not dependencies of core operations.

| Dependency | Stable release checked | Purpose |
|---|---|---|
| clap | 4.6.7 | CLI parsing and derive support |
| serde | 1.0.229 | Typed serialization |
| serde_json | 1.0.154 | JSON decoding and interface output |
| serde_json_canonicalizer | 0.4.1 | RFC 8785 canonical bytes |
| sha2 | 0.11.1 | SHA-256 hashing |
| uuid | 1.29.0 | Asset, issue, and transaction identities |
| toml | 1.1.8+spec-1.1.0 | Repository configuration |
| thiserror | 2.0.21 | Structured domain errors |
| tempfile | 3.27.0 | Atomic publication and test repositories |
| ignore | 0.4.34 | Git-style `.knotignore` patterns |
| unicode-normalization | 0.1.26 | Portable NFC path validation |
| unicode-casefold | 0.2.0 | Portable path collision detection |
| similar | 3.2.1 | Unified text diffs |

Shared requirements live in `[workspace.dependencies]`, with resolver 3 and workspace `rust-version`. `Cargo.lock` records the exact direct and transitive graph; the table is a dated verification record, not a substitute for checking new releases. crates.io's newest publication may be a backport on an older line, so check maximum stable versions rather than assuming the first publication is newest by SemVer.

Before introducing or upgrading a crate:

1. Check its [crates.io](https://crates.io/) page, official documentation, stable versions, Rust requirement, platform support, and required features.
2. Prefer its latest stable major release compatible with the workspace. Select minimal necessary features. Avoid wildcard requirements, prereleases, and moving Git dependencies.
3. Update the shared requirement when changing major versions; `cargo update` alone stays within manifest constraints. Document any reason to remain on an older release.
4. Review `cargo update --dry-run`, perform the intended update, and inspect `cargo tree -d` for unexpected duplication. Do not hand-edit the lockfile.
5. Run format checks, Clippy, workspace tests, and the Linux/macOS/Windows CI matrix before accepting the upgrade. Serialization/hash golden vectors must remain unchanged unless a deliberate format migration is specified.

Normal CI uses `--locked`. A separate disposable Linux job runs `cargo update` and checks/tests the newest compatible dependency graph without modifying the committed lockfile. Check for newer major releases before each milestone; the compatible-resolution CI job cannot discover upgrades excluded by existing requirements.

Core operations have no service dependency. Dependency downloads and CI access the network during development; initialized repository operations do not. The latest compatible graph check does not silently update users' installed binaries or rewrite repository data.

References: [Cargo resolution](https://doc.rust-lang.org/cargo/reference/resolver.html), [Cargo update](https://doc.rust-lang.org/cargo/commands/cargo-update.html), and [Cargo lockfiles](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html).
