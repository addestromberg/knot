---
name: knot-repository-management
description: Inspect and maintain a Knot engineering repository, including local issues, integrity checks, recovery, and peer synchronization.
---

# Knot repository management

Use `knot status --json` before changing repository state. The graph describes
staging; unstaged working edits need `knot add`. Only successful commits advance
HEAD. Each tracked asset has one selected working pathname; history lives in
`.knot/objects/`.

Use `knot issue` for repository-local discussion. Issues survive engineering
checkout and are not verification evidence. Use `--expected-head` when updating
an issue from a previously inspected event.

If recovery is required, inspect the interrupted operation before choosing
`recover --continue` or `--abort`. Preserve external edits that block recovery.
Use `fsck` to verify objects and references.

For synchronization, inspect the named remote, fetch, and use fast-forward-only
pull/push. Discovery is a connection hint, not authorization. Preserve SSH host
verification. Do not mirror a live `.knot/` directory with a file-sync tool.

MCP writes require server startup authorization, a preview, and its expected
snapshot, HEAD, worktree, and reference tokens. Apply existing user authorization; the skill
does not authorize publishing to unrelated peers.
