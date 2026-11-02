# Knot

> **Disclaimer: this is vibe coded beta software. Use at your own risk.**
> Knot was built with substantial LLM assistance and may contain bugs that cause
> data loss or incorrect results. Keep independent backups and verify engineering
> decisions yourself. The software is provided as is, without warranty.

Knot is a command-line version control system for engineering projects, written
in Rust. Keep CAD models, PCB layouts, firmware, software, documentation, and test
procedures together, with a history of how they depend on each other.

Like Git, Knot supports staging, commits, branches, tags, and checkout. It also
tracks dependencies between assets and warns when changed inputs need a fresh
review. Your working directory contains one selected version of each file;
previous versions stay in `.knot/`.

Local work runs offline, without a database, account, or hosted server. Repositories
can synchronize through local paths or SSH, with optional Tailscale connectivity.
Knot currently records manual verification; it does not automatically prove that
parts fit, circuits work, or firmware is compatible.

## Install

### Download a beta build

Open [Releases](https://github.com/addestromberg/knot/releases) and download the
archive matching your operating system and CPU architecture. Windows packages
are ZIP files; Linux and macOS packages are `.tar.gz` files. Each contains `knot`,
the optional `knot-mcp` server, and `BUILD-INFO.json` identifying the source commit.
A `.sha256` file accompanies each archive.

Extract the archive, then run `./knot --version` on Linux/macOS or
`.\\knot.exe --version` in Windows PowerShell. Move the executables to a directory
on your `PATH` to run them from anywhere. Prebuilt downloads do not require Rust.
Choose the source installation below if a build does not support your system.

### Install from source

Install [Rust with rustup](https://rustup.rs/) and Git, then run:

```sh
git clone https://github.com/addestromberg/knot.git
cd knot
cargo install --path crates/knot-cli --locked
knot --version
```

The repository selects its required Rust toolchain automatically through rustup.
Installation downloads dependencies and compiles the application. Make sure
Cargo's binary directory (`~/.cargo/bin`, or `%USERPROFILE%\.cargo\bin` on Windows)
is on your `PATH`.

Knot supports regular files on Linux, macOS, and Windows. Symbolic links and
special files are not supported.

## Start tracking a project

From a directory containing your project files:

```sh
knot init --author "Your name"
knot add .
knot status
knot diff --staged
knot commit -m "Initial project"
```

For later changes, edit your files normally, then stage and commit them:

```sh
knot add mechanical/housing.FCStd
knot status
knot diff --staged
knot commit -m "Update housing"
knot log
```

`knot status` shows file changes, the dependency graph, and verification warnings.
Use `knot mv old-name.step new-name.step` to rename a tracked asset while retaining
its identity and dependencies.

## Branches and checkout

```sh
knot branch new-housing
knot checkout new-housing
# Edit, stage, and commit your changes.
knot checkout main --preview
knot checkout main
knot tag prototype-1
knot checkout prototype-1
```

`branch` creates a branch; `checkout` switches to it. Branch checkout attaches to
that branch, while tags and full commit hashes check out a detached state.
Checkout requires clean staging and tracked working files, and refuses to
overwrite untracked files.

## Ignore generated and local files

Create a `.knotignore` in your project root or a subdirectory:

```gitignore
# Build output, backups, and local configuration
build/
*.bak
*.tmp
.env
.env.*
!.env.example
```

Rules use [Git-style ignore patterns](https://git-scm.com/docs/gitignore), including
wildcards, directory rules, and `!` exceptions. Nested `.knotignore` files can
override parent rules within their directory. Ignored directories must be
re-included before files inside them can be re-included.

Ignored untracked files stay out of `status` and `add`. Already tracked files
remain tracked. Commit your `.knotignore` to share the rules; Knot does not read
`.gitignore`.

## Track dependencies and reviews

For example, record that a PCB depends on the selected housing revision:

```sh
knot depend add electronics/controller.kicad_pcb --on mechanical/housing.FCStd
knot deps electronics/controller.kicad_pcb
knot impact mechanical/housing.FCStd
knot graph
knot commit -m "Record housing dependency"
```

After changing either asset, stage the changes and inspect `knot status`. Previous
reviews may be marked stale, and an outdated revision requirement can block a
commit. Inspect or test the actual design before updating the requirement and
recording a new review:

```sh
knot depend add electronics/controller.kicad_pcb --on mechanical/housing.FCStd
knot verify electronics/controller.kicad_pcb --on mechanical/housing.FCStd \
  --reviewer "Your name" --notes "Checked mounting holes and clearance"
knot commit -m "Review updated assembly"
```

Dependencies can connect any tracked assets, including two mechanical parts,
firmware, or documentation. `knot validate --affected` reports manual test status;
it does not run test scripts. See the [verification guide](docs/verification.md)
for test procedures, recorded results, overrides, and project releases, or the
[resolution guide](docs/resolution.md) to select compatible revisions.

## Synchronize repositories

Create a bare repository on a peer machine with Knot installed:

```sh
knot init /home/engineer/projects/groundstation --bare
```

From your working project, connect using your SSH account and push:

```sh
knot remote add origin ssh://engineer@workshop/home/engineer/projects/groundstation
knot push origin
```

On another machine:

```sh
knot clone ssh://engineer@workshop/home/engineer/projects/groundstation ./groundstation
cd groundstation
knot pull
```

A local directory can replace the SSH URL. SSH authentication and trusted host
keys must already be configured. For LAN discovery or optional Tailscale
connections without port forwarding, see the [peer synchronization guide](docs/synchronization.md).

## Keep project issues alongside the files

```sh
knot issue create --title "Check PCB clearance" --body "Measure the mounting holes."
knot issue list
knot issue show <issue-id>
knot issue comment <issue-id> --body "Measurements completed."
knot issue close <issue-id>
```

Issues synchronize with peers and remain current when you check out older project
snapshots.

## Use with LLM agents

Install the optional MCP server from the cloned Knot source directory:

```sh
cargo install --path crates/knot-mcp --locked
knot-mcp -C /path/to/your/project
```

The server starts with read-only access. Start it with `--allow-writes` to permit
explicitly applied changes through its preview and authorization checks. See
[MCP setup and skills](docs/mcp.md) for agent-host configuration.

For scripts, use machine-readable output such as `knot status --json`.

## Restore and recover

```sh
knot restore mechanical/housing.FCStd --preview
knot restore mechanical/housing.FCStd
knot fsck
```

`restore` replaces the selected working file with its staged contents. Inspect
the preview before discarding local edits. If an interrupted operation requires
recovery, follow `knot status` and use `knot recover --continue` or
`knot recover --abort`.

## Help and feedback

Run `knot --help` or `knot <command> --help` for command options. Report bugs and
beta feedback in [GitHub Issues](https://github.com/addestromberg/knot/issues).
