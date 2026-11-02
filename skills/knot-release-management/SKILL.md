---
name: knot-release-management
description: Prepare and record a policy-checked named release from an exact committed Knot snapshot.
---

# Knot release management

Inspect the committed snapshot, dependency requirements, verification state, and
tracked `knot-policy.json`. The default release policy requires current dependency
reviews and all defined tests. Additional mandatory tests and asset reviews may
be configured in that versioned file.

Resolve outstanding reviews and declared conflicts before release. Report
accepted overrides explicitly; commits using override evidence carry `(raped)`
and retain the underlying immutable record IDs.

Use `release create <name> --approver ... --notes ...` on clean tracked state.
Release names are immutable; a release binds its commit, snapshot, policy, and
evidence. Use `checkout release/<name> --preview` to inspect restoration and
`checkout <target> --validated` to require applicable current policy evidence.

Creating a local release does not authorize pushing to an external peer.
