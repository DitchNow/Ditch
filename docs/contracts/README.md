# Community Commercial Upgrade Contracts

`commercial-offers-v1.json` is the canonical provider-neutral example for the
Commercial offer catalog consumed by Community `ditchd` and the macOS app.
`commercial-offers-v1.schema.json` defines the corresponding strict wire shape.

These files contain upgrade discovery only. They must not contain Stripe object
identifiers, billing secrets, Remote Control protocol details, pairing, relay
transport, projections, or mobile command implementation.

Run the release-blocking check from the repository root:

```sh
scripts/check-commercial-offer-contract
```

Export a deterministic bundle for a private Relay checkout with:

```sh
scripts/export-commercial-offer-contract /path/to/output-directory
```

The export manifest records the exact Community Git revision and SHA-256 digest
of each contract artifact. Export deliberately requires the contract files to
be committed and clean so the revision cannot misrepresent their contents.
Relay CI can compare its generated/copied contract to an explicitly pinned
Community checkout without making Community depend on the private Relay
repository.
