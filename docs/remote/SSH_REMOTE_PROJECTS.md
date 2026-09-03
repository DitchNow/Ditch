# SSH Remote Projects

Remote projects use the same `Project`, `AgentRun`, event store, transcript,
attention, Codex, and Remote Protocol implementation as local projects. A
project's `execution_target` is either `local` or a remote machine identity,
SSH alias, and target-owned root path. The root is interpreted only on its
execution target.

## Authority and transport

Flutter sends typed requests to the local `ditchd` Unix socket. Local `ditchd`
resolves OpenSSH configuration and owns administrative SSH processes. Normal
remote project requests travel over one persistent per-host stdio bridge:

```text
Flutter -> local ditchd -> system ssh -> ditchd bridge --stdio
                                      -> ~/.ditch/run/ditchd.sock
                                      -> persistent remote ditchd
```

The bridge is transport only. It cannot execute a client-supplied shell
command, and disconnecting it does not stop the remote daemon, PTY, or Codex.
Mutating requests are never automatically retried. Reconciliation polls the
remote daemon's epoch and authoritative snapshot; a failed poll preserves the
last in-memory projection and marks only connectivity offline.

Bootstrap uses fixed scripts and argument-array OpenSSH processes for platform
detection, checksummed artifact installation, user-service installation, and
dependency setup. Source trees are never copied, mounted, mirrored, or opened
through a generic remote file API.

## SSH security

`ssh -G` is authoritative for effective HostName, User, Port, identities, and
authentication policy. Concrete aliases are discovered from `~/.ssh/config`
and Includes. ProxyJump, ProxyCommand, wildcard-only targets, and
keyboard-interactive-only authentication are rejected in v1.

The connection policy adds only public-key/password preference,
keyboard-interactive disablement, prompt limits, and a connect timeout. It
never disables strict host-key checking. Unknown fingerprints come from the
actual OpenSSH askpass prompt and require confirmation; changed keys stop.

Passwords exist only in the askpass broker's memory unless the user elects to
store the connection-scoped credential in macOS Keychain. They are never sent
as arguments or environment values and are never written to SQLite, SSH
config, logs, or files. Key passphrases remain the SSH agent's responsibility.

## Remote runtime

The per-user singleton lives under `~/.ditch` with 0700 directories and 0600
sensitive files. Linux uses a systemd user service and requires lingering for
logout persistence. macOS uses a user-domain LaunchAgent. No Ditch component
requires root; Ditch never silently invokes sudo.

Missing systemd lingering is shown as a persistence warning but does not block
using a daemon that is currently running. The project wizard can continue;
the user is told that the daemon may stop when the remote account logs out.
Once required checks pass, the wizard automatically replaces the checklist
with a directory-only browser rooted initially at the remote user's `$HOME`;
no extra "Check Again" action is part of the successful path.

Runtime artifacts are exact-version, target-qualified, SHA-256 verified, and
installed into a version directory before an atomic `current` symlink switch.
The previous symlink is retained for rollback. Updates refuse to proceed while
the daemon reports active sessions. `scripts/build-remote-artifacts` emits the
release artifacts and manifest; the app packages its native macOS Rust daemon
artifact separately from the Swift status-host executable.

Remote machine identity is generated on the remote host. macOS stores its
private identity in Keychain; headless Linux uses an atomic 0600 file under the
0700 `~/.ditch/identity` directory. Identity material is never copied to the
Mac. The existing iPhone pairing protocol can bind the remote machine to the
same owner: an already-authorized iPhone scans the remote machine's one-time
pairing QR, then the remote daemon maintains its own outbound Cloudflare
connection. iPhone control does not traverse the Mac.

Mobile enrollment is optional and lives in Remote / Mobile settings. It is
not part of SSH project onboarding and never blocks browsing or registering a
remote project from the desktop app.

## Deliberate boundaries

- Remote Linux/macOS targets are limited to x86_64 and arm64 GNU Linux and
  current Rust-supported macOS targets.
- No Windows, ProxyJump/ProxyCommand, bastions, keyboard-interactive/PAM/MFA,
  SSHFS, rsync, generic remote shell, remote source editor, or offline command
  queue is provided.
- The interactive PTY exposed during setup can launch only the fixed remote
  `codex login` flow. The remote daemon owns that PTY.
- Linux Git installation remains manual when elevation is required; exact
  package-manager instructions are shown. User-owned Homebrew installation is
  the only automatic Git path in v1.
- Worktrees are not implemented in this repository. The execution-target
  boundary ensures a future worktree manager runs in the daemon that owns the
  project and Git repository.
