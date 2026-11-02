# Manual verification, traceability, and releases (M4)

Knot records what a person inspected against exact selected inputs. It does not
extract CAD geometry, execute external test programs, or infer physical fit.
Agents can coordinate inspection through the same interfaces, but no agent is
required.

## Review a dependency

```sh
knot add pipe.step
knot status
# Inspect the assembly, then update its declared requirement if appropriate.
knot depend add rod.step --on pipe.step
knot verify rod.step --on pipe.step --reviewer "Engineer" \
  --notes "External mount leaves bore and clearance unchanged"
knot commit -m "Add pipe mount after fit review"
```

A dependency review binds both endpoint revisions and all requirements between
them. An asset review (`verify <asset>` without `--on`) binds that revision.
Input files must match staging. Records include reviewer, outcome, UTC start/end
times, environment, notes, artifacts, and an immutable history parent. Manual
recording currently stamps both times at attestation; include the measurement
period in notes when needed. Reviewer
names are descriptive, not cryptographic approvals.

Evidence is stored independently under `refs/reviews/`; recording it does not move
HEAD or change engineering state hashes. Status derives `unverified`, `verified`,
`stale`, `failed`, or `error`. Changed endpoint revisions make old evidence stale
even when a named interface declaration stays identical. Upstream warnings refer
to unresolved dependency reviews; unrelated local test results remain applicable.
Historical checkout can reuse earlier evidence for the same exact inputs. Newer
failed/error results for those same inputs supersede earlier passes.

## Manual test definitions and results

```sh
knot tests define sliding-fit --procedure tests/sliding-fit.md \
  --input pipe.step --input rod.step --description "Measure insertion clearance"
knot commit -m "Define sliding-fit inspection"
knot tests list
knot validate --affected
# Execute the procedure manually, then record the actual result:
knot validate sliding-fit --record passed --reviewer "Engineer" \
  --notes "Measured 0.3 mm clearance" --environment "Bench A, gauge set 4" \
  --artifact reports/measurements.txt
knot tests history sliding-fit
```

`validate` without `--record` reports state; it does not execute a script. Outcomes
are `passed`, `failed`, and `error`. The generated
`tests/<name>.knot-test.json` manifest is an ordinary tracked asset referencing an
immutable TestDefinition. Evidence binds the manifest revision, procedure
revision, and all declared input revisions. Include configuration/toolchain
descriptions as tracked inputs when changes to them must invalidate evidence.
Changing a definition or procedure therefore requires renewed evidence. Artifacts
are retained as immutable blobs even if their working files are later removed.

## Explicit override command

`knot rape` prompts separately for each dependency relationship. Accepted
relationships are rebound to the selected target revision/interface and receive
audited override records. Revision ranges become exact selected-revision pins.
Declining leaves that relationship unaccepted. `--forced` accepts every guard:

```sh
knot rape --forced --reviewer "Operator" --notes "Explicit acceptance of this WIP"
knot commit -m "Record accepted configuration"
knot log
```

Status labels accepted overrides `(raped)`. Every commit using applicable override
evidence stores those record IDs and displays `(raped)` in human history. The
marker survives later review changes and synchronization. Subsequent commits that
still use that evidence also carry it; a genuine replacement review ends its use
for future commits. Override records preserve their original snapshot and exact
inputs and explicitly state that no engineering test was claimed. They do not
alter test-result history. Missing assets/interfaces and impossible revision-pin
cycles require explicit graph repair; the command cannot invent a selected target.
Stage relevant working edits first. Earlier accepted records are never erased.

## Traceability and release policies

```sh
knot trace rod.step --to docs/REQ-042.md --relation satisfies
knot trace tests/sliding-fit.md --to docs/REQ-042.md --relation verifies
knot trace docs/REQ-042.md --reverse --transitive
knot release create v1.0 --approver "Engineer" --notes "Fit reviewed"
knot checkout release/v1.0 --preview
knot checkout main --validated
```

Relations are `documents`, `describes`, `satisfies`, or `verifies`. Generated
`docs/trace/*.knot-link.json` manifests follow commits. Links bind exact source and
target revisions; queries report `current: false` for potentially outdated links.

Track `knot-policy.json` to configure releases:

```json
{
  "require_dependency_reviews": true,
  "require_all_tests": true,
  "required_tests": ["sliding-fit"],
  "required_assets": []
}
```

These are also the default booleans when no policy file exists. Required assets
use full stable asset IDs. Release creation requires clean, committed tracked
state, satisfied declarations, and applicable passed evidence for the policy.
Explicit override reviews satisfy dependency-review gates and remain marked and
auditable; they do not satisfy separately required tests. A release name is
immutable and binds its commit, snapshot, policy, evidence IDs, approver, and time.
Integrity checks verify the release's own historical evidence, independently of
later reviews. Untracked files are outside a release's snapshot.
