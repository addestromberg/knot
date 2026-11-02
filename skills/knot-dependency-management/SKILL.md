---
name: knot-dependency-management
description: Declare and inspect Knot asset dependencies, interface requirements, change impact, and compatible revision resolution.
---

# Knot dependency management

Inspect `knot graph --json`, `deps <asset> --reverse --transitive`, and
`impact <asset> --since HEAD`. Dependencies use stable asset identities; use
`knot mv` to retain them through renames.

`depend add <owner> --on <target>` pins the selected revision. Named interface
requirements are explicit contracts; matching declarations do not prove the
modified engineering file still conforms. An unchanged interface can preserve
declared compatibility while the corresponding manual review becomes stale.

Use `--min-revision` and optional `--max-revision` for inclusive ancestry bounds,
not numeric labels or semantic-version assumptions.

Preview `knot resolve` with explicit `--pin asset@full-hash` selections. Inspect
every changed asset before `--apply`; application stages the resolved snapshot
without advancing HEAD. Missing assets, conflicting requirements, and bounded
search exhaustion must be reported rather than relaxed silently.
