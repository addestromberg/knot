# Repository Guidelines

## Project Structure & Architecture

Knot is an offline-first engineering version control system. All backend code is Rust; a future frontend will use React. `crates/knot-core/` owns domain models, storage traits, and repository operations. `crates/knot-store/` implements filesystem storage; `crates/knot-cli/` provides the `knot` executable. Future MCP and server adapters must reuse these libraries. Architecture and format decisions live in `docs/`.

Repository state lives in `.knot/`, without a database or required service. Objects are immutable, schema-versioned, and content-addressed. Snapshot hashes exclude issue activity and validation evidence. Local issues have independent immutable history and survive engineering checkouts. M2 adds versioned dependencies, exact revision/interface requirements, reverse traversal, and impact analysis. Keep automatic resolution, validation engines, MCP, remotes, and React outside M2. Engineering requirements are declarations, not evidence that physical tests passed.

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

Add behavior and regression tests alongside changes. Use temporary repositories, deterministic hash vectors, copied-repository reconstruction, concurrency checks, and interrupted-transaction recovery tests. Preserve asset IDs across explicit renames. Never overwrite dirty files or untracked checkout destinations. Publish verified objects before advancing refs; keep mutations locked and recoverable. Preserve schema-1 hashes; engineering metadata uses revision schema 2. Commit and checkout reject unsatisfied requirements unless explicitly overridden.

## Commits & Pull Requests

Use focused commits with imperative subjects, such as `Add snapshot integrity verification`. Pull requests explain behavior, link relevant issues, and list validation and platform limitations. Never commit credentials or private local configuration.
