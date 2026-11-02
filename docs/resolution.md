# Compatible revision resolution (M3)

`knot resolve` previews a complete compatible configuration of the assets already
selected in staging. It selects one exact revision per asset and retains current
paths. It does not claim physical compatibility or validation.

```sh
knot resolve
knot resolve mechanical/pipe.step@<full-revision-hash>
knot resolve --pin mechanical/pipe.step@<hash> --pin firmware/main.rs@<hash>
knot resolve mechanical/pipe.step@<hash> --apply
# Equivalent asset-driven checkout; preview is the default:
knot checkout mechanical/pipe.step@<hash> --resolve
knot checkout mechanical/pipe.step@<hash> --resolve --apply
```

## Constraints and deterministic policy

Exact revision requirements and named interface versions retain their M2 meaning.
Inclusive ancestry ranges accept descendants of a minimum revision, optionally
restricted to ancestors of a maximum:

```sh
knot depend add rod.step --on pipe.step --min-revision <hash> --max-revision <hash>
```

These are DAG ancestry bounds, not numeric revision labels or semantic versions.
Both bounds must belong to the target; the maximum must descend from the minimum.

Candidates come from staging, reachable local/tag/remote commit history, revision
parents, exact requirements, and explicit pins. Unrelated abandoned staging
objects are excluded. Resolution preserves the current dependency relationship
keys; it cannot silently erase a relationship by choosing a revision from before
that relationship existed. Use exact historical checkout to restore another graph.

Order assets from current dependency roots through their targets, with path order
for roots/siblings and cycle-safe traversal for remaining components. Within each
asset, prefer larger distinct ancestor counts, then descending revision hash.
Backtrack to the first complete satisfying assignment. Explicit pins are fixed.
This deterministic lexicographic policy does not optimize manufacturing cost,
timestamps, or an inferred engineering version number.

The search stops after 100,000 candidate attempts. `resolution_conflict` means no
solution was found after exhausting candidates; `resolution_limit` means the
search bound was reached and does not prove impossibility. JSON details identify
conflicting requirements and the attempted target selection.

## Applying a preview

Preview computes snapshot IDs and changes without writing objects, refs, staging,
or working files. Application recomputes the result and checks expected HEAD,
staging, clean tracked files, and untracked destinations. Existing locking and
recovery rules apply. The resolved configuration is staged; HEAD advances only
when you commit. Reviews of changed inputs become stale independently of whether
all declared constraints match.
