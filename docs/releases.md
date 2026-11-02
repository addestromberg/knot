# Beta builds and downloads

Every successful push or merge into `main` automatically creates a GitHub beta
prerelease. CI first runs formatting, Clippy, builds, and tests on Linux, macOS,
and Windows, plus the latest-compatible dependency checks. Any failed job blocks
publication. Development pushes and pull requests do not trigger CI automatically.

The tag is `v<workspace-version>-beta.<workflow-run-number>`, for example
`v0.6.0-beta.12`, and points at the exact tested commit. Each platform archive
contains the release-mode `knot` and `knot-mcp` executables and `BUILD-INFO.json`.
The archive name includes the native Rust target triple, so users can select the
correct OS and CPU architecture. A separate `.sha256` file verifies each archive.
Beta tags distinguish packaged builds; the binaries' `--version` reports the
underlying workspace version.

## Download and use

Open [Releases](https://github.com/addestromberg/knot/releases), select a beta, and
download its platform archive under **Assets**, plus the matching checksum file.
Extract the ZIP on Windows, or the `.tar.gz` archive on Linux/macOS. Run
`.\\knot.exe --help` in PowerShell or `./knot --help` in a Unix terminal, then put
the executables on your `PATH` if desired. Rust is not required for these builds.

For a Unix checksum check, run `sha256sum -c <archive>.sha256` (macOS can use
`shasum -a 256 -c <archive>.sha256`). In PowerShell, use
`Get-FileHash <archive> -Algorithm SHA256` and compare it with the checksum file.

## Verify development before merging

Use **Actions → Rust → Run workflow**, choosing `development`, or run:

```sh
gh workflow run ci.yml --ref development --repo addestromberg/knot
```

Manual runs execute the same platform checks and packaging but never create a
release. Their downloads appear under the run's **Artifacts**, with 30-day
retention. GitHub requires sign-in for artifact downloads. Published release
assets remain available independently of artifact expiration.

Publication uses GitHub's automatic token with write permission scoped to the
release job; test jobs retain read-only repository access. Re-running a completed
main build replaces that run's assets only if the existing tag still identifies
the same commit. The workflow does not merge pull requests or advance `main`.
