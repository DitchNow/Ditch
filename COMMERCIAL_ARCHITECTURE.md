# Commercial source boundary

This private repository owns iPhone Remote Control, Relay connectivity, pairing, mobile command encryption, mobile projections, paid capability enforcement, and future proprietary capabilities.

`community/` is a Git submodule pinned by `COMMUNITY_REVISION`. Commercial CI must run `scripts/verify-community-pin`, then run the complete Community test suite from that submodule before building private source.

The Commercial application and SSH artifacts contain exactly one executable runtime named `ditchd`. Both edition roots include the same Community-owned `runtime_shared.rs`; the private `edition.rs` statically injects capability state, schema initialization, lifecycle hooks, and private request routing. It must never add a second daemon, database authority, PTY owner, session authority, or process supervisor.

The Flutter application is a thin private composition root. It depends on `community/apps/macos` and injects `CommercialEditionSurface`; proprietary pairing and device UI lives only in `commercial_remote_surface.dart`. It does not carry a copied patch stack of Community `main.dart`.

Commercial `ditchd` reuses the exact Community installation identity, refreshes sanitized entitlement from the official backend, and gates only private capability routes and Relay connectivity. The local macOS runtime migrates the legacy `identity/installation-v1.json` private material into the login Keychain after a verified write; headless SSH runtimes retain their protected `~/.ditch` file identity. Relay remains authoritative. Expiry removes `remote_control_v1` from runtime capabilities and closes the Relay path while local sessions, PTYs, persistence, and SSH remain available.

New Community functionality must be consumed from the pinned submodule rather than copied into private UI files. Private protocol contracts remain canonical here and are exported deterministically to the private Relay and iPhone repositories. Commercial-only SQLite tables live behind `CommercialStoreExt`, while the canonical Community `DitchStore` retains the only connection and migration authority.
