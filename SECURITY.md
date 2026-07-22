# Security

Temporal is a local, single-user macOS tool. It runs entirely on your machine:
no network services, no cloud, no telemetry. The only outbound network access
is the one-time model download in `build/fetch-models.sh` (pinned by SHA-256).

## Threat model

The daemon (`temporald`) captures and later reconstructs desktop state. It holds
data about your open apps, windows, browser tabs, terminal working directories,
and editor projects in a SQLite database under
`~/Library/Application Support/temporald/`.

The design assumes a single trusted user on the machine. The boundary that
matters is **process-to-process on the local host**, not the network.

## Trust boundary

The real access-control boundary is the IPC surface:

- The Unix domain socket is created **owner-only** — a restrictive `umask` makes
  it inaccessible to group/other at creation time (no race window), then it is
  chmod'd to `0600`. Its parent directory is `0700`.
- Every accepted connection's **peer uid is checked** against the daemon's own
  uid (`SO_PEERCRED` via `UnixStream::peer_cred`); a different user's process is
  rejected before any request is read.
- Frames are length-bounded (16 MiB) and concurrent connections are capped, so a
  local misbehaving client cannot exhaust memory or spawn unbounded work.

Given that boundary, **any same-uid process that can reach the socket is trusted
to request rehydration of arbitrary content.** The normal client is the bundled
Tauri UI, which round-trips workspaces the daemon itself produced.

### Why rehydration payloads are not verified against storage

Rehydration requests carry a full workspace payload. Two of the three candidate
kinds the query engine returns — semantic *groups* and prompt-*assembled*
workspaces — are synthesized on the fly and never persisted verbatim under a
matching id, so "reject unless every node exists in storage" would break
legitimate flows. Instead the daemon applies **defense-in-depth on the sinks**,
not provenance checks:

- **Browser tabs**: only allowlisted URL schemes are reopened
  (`http`, `https`, `chrome`, `chrome-extension`, `about`); `file:`,
  `javascript:`, and `data:` are dropped.
- **App launch**: bundle identifiers must have a valid reverse-DNS shape before
  reaching `open`/`mdfind`, rejecting flag- and quote-injection shapes.
- **Subprocess arguments**: all launches use `Command::args` (no shell), with a
  `--` separator so a leading-`-` path can't be parsed as a flag.
- **Payload size**: node and per-node tab counts are capped before any work.

These are hardening measures that reduce blast radius, **not** a privilege
boundary. The privilege boundary is the owner-only, peer-checked socket above.

## Model integrity

`build/fetch-models.sh` downloads the embedding and tagging models over HTTPS
(`--proto '=https'`) and verifies each against a pinned SHA-256 before use.
Verification is unconditional: an entry without a pinned digest fails closed.

## What is intentionally out of scope

- **App sandbox** for the daemon: it relies on Accessibility, Apple Events, and
  window-list APIs that are incompatible with the App Sandbox, so it is not
  sandboxed. It runs as a per-user LaunchAgent with only the permissions the
  user grants (Screen Recording, Accessibility).
- **Multi-user / networked** deployment: unsupported by design.

## Reporting

This is a personal open-source project. Please open a GitHub issue for security
concerns; for anything you'd rather not disclose publicly, note that in the
issue and a private channel can be arranged.
