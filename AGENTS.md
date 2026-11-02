# Repository Guidelines

## Project Structure & Architecture

Knot is an offline-first engineering version control system. Use Rust; the CLI is the product interface, with MCP for agents. `crates/knot-core/` owns domain models, storage traits, and repository operations. `crates/knot-store/` implements filesystem storage; `crates/knot-cli/` provides `knot`; `crates/knot-mcp/` provides the stdio agent server. Adapters must reuse these libraries. Design decisions live in `docs/`.

Repository state lives in `.knot/`, without a database or required service. Objects are immutable, schema-versioned, and content-addressed. Snapshot hashes exclude issue activity and validation evidence. Local issues have independent immutable history and survive engineering checkouts. M3–M6 add deterministic resolution, manual verification, traceability, releases, MCP/skills, and peer synchronization. Keep geometry extraction and external test runners optional. Matching declarations do not prove engineering compatibility. Operator overrides retain audit records and mark commits `(raped)`.

## Build, Test, and Development Commands

- `cargo build --workspace --locked`: build all crates with recorded dependencies.
- `cargo run -p knot-cli -- --help`: inspect the CLI.
- `cargo test --workspace --locked`: run domain, filesystem, and CLI tests.
- `cargo fmt --all -- --check`: verify formatting.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: enforce lint checks.

## Coding Style & Dependencies

Use Rust 2024, rustfmt's four-space indentation, `snake_case` functions/modules, and `UpperCamelCase` types. Prefer typed identifiers, structured errors, and small synchronous operations. Avoid panics in production paths.

Before adding or upgrading dependencies, verify the latest stable releases on crates.io and official documentation. Select the newest mutually compatible versions for the pinned stable toolchain and supported platforms. Centralize requirements in `[workspace.dependencies]`, minimize features, and commit `Cargo.lock`. Document compatibility exceptions. Review new major versions explicitly; never silently change canonical bytes or repository schemas during an upgrade.

## Testing & Safety

Add behavior and regression tests alongside changes. Use temporary repositories, deterministic hash vectors, copied-repository reconstruction, concurrency checks, and interrupted-transaction recovery tests. Preserve asset IDs across explicit renames. Never overwrite dirty files or untracked checkout destinations. Publish verified objects before advancing refs; keep mutations locked and recoverable. Preserve legacy hashes; new schemas require explicit capability markers. Commit and checkout reject unsatisfied requirements unless explicitly overridden. Synchronization verifies objects before journaled, fast-forward-only ref updates. Mesh services remain optional; discovery hints never grant access.

## Commits & Pull Requests

Use focused commits with imperative subjects, such as `Add snapshot integrity verification`. Pull requests explain behavior, link relevant issues, and list validation and platform limitations. Never commit credentials or private local configuration.
