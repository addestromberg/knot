---
name: knot-revision-management
description: Edit, stage, review, commit, and restore exact engineering configurations in Knot.
---

# Knot revision management

Inspect status and the dependency graph, edit the selected working files, then
stage intended paths. Review `diff --staged`, requirement conflicts, and stale
verification before committing. Documentation and test manifests are ordinary
versioned assets and must be staged explicitly.

Create a branch with `branch <name>`, then switch using `checkout <name>`.
Tags and commit IDs detach HEAD. Exact checkout restores a whole recorded
configuration; it does not independently choose each asset's latest revision.

Use checkout/restore previews before replacing files. Dirty tracked files and
untracked destination collisions require an explicit user decision about that
work; never delete them to make checkout succeed.

`commit --allow-incompatible` records deliberate WIP. The override-review command
`rape` is a different operation: it rebinds accepted dependencies, records override
evidence, and marks the resulting commit `(raped)`. Do not use it merely to clear
warnings during ordinary revision work.
