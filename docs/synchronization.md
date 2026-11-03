# Peer synchronization and optional mesh networking (M6)

Knot is a CLI application. Peers exchange immutable objects and guarded refs;
there is no required hosted platform, UI, database, discovery service, or VPN.

## Local paths and SSH

```sh
# A bare peer accepts pushes without a checked-out working branch.
knot init /srv/knot/project --bare
knot remote add origin /srv/knot/project
knot push origin
knot clone /srv/knot/project ./copy
knot fetch origin
knot pull origin

# The destination must have Knot installed and authorize this SSH account.
knot remote add workshop ssh://engineer@workshop/home/engineer/project
knot push workshop
knot remote list
knot remote set-url workshop ssh://engineer@new-address/home/engineer/project
```

Local paths, `file://` paths, `ssh://user@host[:port]/absolute/path`, and optional
`tailscale://user@host/absolute/path` addresses are supported. The SSH endpoint
uses a POSIX-compatible remote shell with Knot available in PATH; Windows can
act as a client to such peers and use filesystem remotes. Local CLI URLs are
normalized to absolute paths. Clone requires an empty destination; specify
`--branch <name>` to select a nondefault branch. An interrupted/failed clone may
leave initialized metadata for inspection rather than deleting local data.

OpenSSH handles keys, agents, account authorization, encryption, and host
verification. Batch mode and strict known-host checks are enabled. Establish the
host's trusted key through your SSH setup before unattended synchronization.
The remote command is a quoted `knot -C <path> serve --stdio` endpoint; pushes
explicitly request `--allow-writes`. A restricted SSH account can use a fixed
forced command/path and filesystem permissions to limit accessible repositories.
Read-only endpoints reject uploads. Remote configuration stays local and is not
exchanged as engineering history.

For a dedicated read-only key, combine the fixed command with OpenSSH's
`restrict` option; a forced command alone does not disable forwarding:

```text
restrict,command="/usr/local/bin/knot -C /srv/knot/project serve --stdio --idle-timeout 60" ssh-ed25519 <public-key>
```

The CLI endpoint and SSH/Tailscale clients impose a 60-second deadline on stalled
protocol reads and writes, including partial control messages, raw object frames,
and blocked output. Timeout errors release the repository lock and roll back
incomplete imports. Set `KNOT_SYNC_IDLE_TIMEOUT_SECONDS` (1–86400) to adjust the
client and default endpoint timeout, or use `serve --idle-timeout <seconds>` for a
fixed endpoint policy. The client passes its configured value to the normal SSH
command; a forced command controls its own value. Allow more time for slow links
or peer integrity checks. These application deadlines are separate from SSH's
connection timeout and transport keepalives.

## Transfer and conflict rules

Peers advertise object IDs and committed refs. Only missing objects are sent;
binary objects stream in bounded buffers. Every incoming envelope, schema,
length, canonical structured payload, and SHA-256 ID is verified. Full graph
integrity is checked before refs become visible. New schema readers are required
when newer objects are exchanged.

Fetch writes `refs/remotes/<peer>/<branch>` and imports immutable tags/releases
and independent issue/review histories without changing working files or HEAD.
Push sends the selected/current branch plus tags, issues, reviews, and releases.
Clone includes all advertised branches and materializes one selected snapshot.
Uncommitted working edits are never another peer's checkout state.

Commit refs, issues, and reviews accept fast-forwards only; tags and releases
cannot be replaced. Divergent histories return conflicts, including independent
issue/review forks. There is no automatic merge or force-push. Pull performs fetch
then a guarded fast-forward checkout of the attached branch. A dirty working tree,
untracked destination collision, or incompatible target blocks checkout; explicit
`--allow-incompatible` supports intentional WIP. Pushing into an active working
branch is refused; push to a bare peer or another branch instead.

Exclusive OS locks, expected ref values, and the existing journal protect ref and
working-file publication. Interrupted publication uses `recover --continue` or
`--abort`. Interrupted object transfers roll back newly imported objects using a
durable `incoming.json` record; after a killed process, the next repository open
performs this rollback automatically. Existing history and refs are preserved.
Retry the original push or fetch normally. Complete, graph-verified batches may
remain after a refused ref update; no history is
silently lost. All missing objects are exchanged, including retained orphan
objects; reachable-only packs, chunk deduplication, quotas, and partial clone are
future optimizations.

## LAN discovery

```sh
knot peer announce --name workshop --url ssh://engineer@workshop/home/engineer/project
knot peer discover --seconds 5
knot peer trust workshop ssh://engineer@workshop/home/engineer/project
knot peer list
```

Announcements are opt-in IPv4 multicast on `239.255.75.78:47568`, TTL 1. They
advertise a chosen name, connection URL, and short expiry, not objects or private
keys. One listener per host/port is supported. Discovery results are untrusted
connection hints and never create trusted remotes or trigger downloads. Access
still requires transport authentication. Firewalls/network isolation can prevent
discovery; explicit URLs continue to work.

## Automatic WireGuard connectivity through an optional provider

For operation without router port forwarding, install the optional Tailscale CLI
and daemon on the participating devices, then authorize each device once:

```sh
knot peer mesh-up --ssh
# Optional self-hosted compatible coordinator:
knot peer mesh-up --ssh --login-server https://your-headscale.example
knot peer discover --mesh
knot peer connect workshop --host workshop --user engineer \
  --repository /home/engineer/project
knot push workshop
```

This adapter delegates WireGuard configuration, endpoint discovery, NAT
traversal, and encrypted relay fallback to the provider. Knot never installs or
reconfigures a VPN during ordinary repository commands. `mesh-up` explicitly
invokes provider setup and reports its authentication/daemon requirements.

`tailscale://` uses `tailscale ssh`, including the provider's authenticated host
key lookup. Provider network/SSH access policies must permit the account and
peer. Tailscale SSH server support has platform limits; ordinary SSH over an
existing mesh address is another option. The adapter does not disable host
verification or run privileged network commands through core operations.

LAN discovery alone cannot make two unreachable NAT endpoints reconnect after
they change networks. The optional provider supplies coordination/relay services;
there is no such service required for local, LAN, or reachable SSH peers. Live
internet connectivity requires real authorized devices and is separate from
repository/protocol integration tests.

References: [OpenSSH](https://man.openbsd.org/ssh),
[WireGuard keepalives](https://www.wireguard.com/quickstart/),
[Tailscale connection paths](https://tailscale.com/docs/reference/connection-types),
[Tailscale SSH](https://tailscale.com/docs/features/tailscale-ssh).
