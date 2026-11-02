---
name: knot-validation
description: Define manual Knot engineering checks, record exact-input evidence, and review stale or upstream verification warnings.
---

# Knot validation

Use `knot validate --affected` to inspect checks needing attention. The command
does not run CAD tools or manufacture evidence. Perform the actual inspection or
measurement, or report that it remains unverified.

Define a test using `tests define <name> --procedure <tracked-path> --input <path>`
with every relevant asset/configuration input. Commit the generated test manifest
with its procedure. Changed definitions, procedures, or declared inputs make old
results stale.

Record results using `validate <name> --record passed|failed|error --reviewer ...
--notes ... --environment ...`, optionally attaching artifact files. For a
dependency use `verify <asset> --on <target>`. Stage reviewed input changes first.

Evidence remains independent of HEAD and snapshot hashes. Preserve failed and
superseded records. Distinguish direct stale reviews from upstream warnings.
Author/reviewer text describes provenance; it is not cryptographic identity.
Operator overrides are explicitly audited and do not claim a test was executed.
