# MCP and agent skills (M5)

Build/install the dedicated Rust server:

```sh
cargo install --path crates/knot-mcp --locked
knot-mcp -C /path/to/project
# Enable application of agent mutations explicitly:
knot-mcp -C /path/to/project --allow-writes
```

The server speaks newline-delimited JSON-RPC over stdio. Stdout contains only
protocol messages; stderr carries diagnostics. It supports the current
[2026-07-28 stateless protocol](https://modelcontextprotocol.io/specification/2026-07-28/basic)
and the [2025-11-25 initialization protocol](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).
Modern requests include protocol version and client capabilities in `params._meta`;
`server/discover` advertises supported versions. Legacy clients initialize and send
`notifications/initialized`. Requests are bounded to 8 MiB; EOF closes the server.

## Tools and boundaries

Read operations expose repository info, status/graph, history/assets,
dependencies/impact, verification results, traceability, and issues. Mutating
tools cover resolution, checkout, UTF-8 writes, staging, commits, dependency
declarations, manual test/review evidence, and releases. `tools/list` publishes
their schemas and behavior annotations. All operations call the same Rust core as
the CLI; the MCP server does not invoke shell commands to modify repositories.

Mutating tools default to preview. A preview returns `expected_snapshot`,
`expected_head`, `expected_worktree`, and `expected_refs`. To apply, repeat the operation arguments
with `apply: true` and all four expected values, including a null HEAD before the
first commit. The server must also have been started with `--allow-writes`.
Changed working bytes, staging, HEAD, or any reference (including reviews and fetched history) invalidate the preview. Core dirty-file,
dependency, integrity, recovery, and release policy checks remain enforced.

`knot_run_validation` records a manual test execution; it never claims that an
external tool ran. The host must provide the real reviewer and measured result.
Client identity metadata is not authorization. Operator overrides are available
through the explicit CLI command rather than a hidden MCP bypass.

## Example host configuration

```json
{
  "mcpServers": {
    "knot": {
      "command": "knot-mcp",
      "args": ["-C", "/path/to/project"]
    }
  }
}
```

Use the host's own authorization controls when enabling writes. Review previews
before application; an expected-state token is a consistency guard, not a secret.

The repository ships five self-contained skills under `skills/`:
`knot-repository-management`, `knot-dependency-management`,
`knot-revision-management`, `knot-validation`, and `knot-release-management`.
Install the desired folders in your agent host's skill directory. Skills describe
workflows; integrity and policy are enforced in core operations.
