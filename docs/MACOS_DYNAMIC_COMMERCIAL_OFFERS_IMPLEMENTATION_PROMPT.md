# Implementation Prompt — Dynamic Relay-Sourced Commercial Offers in The Ditch macOS

> Private Commercial implementation material. Keep this document in the Commercial repository.

## Repository and scope

Implement this prompt in:

```text
/Users/mtn/Documents/Personal/The Ditch v2
```

This prompt is exclusively for The Ditch macOS application, Community `ditchd`, and public upgrade bootstrap. Do not edit the Relay repository while executing it.

The strict client contract fixture is `docs/contracts/commercial-offers-v1.json`. The Relay implementation prompt lives at `/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay/docs/RELAY_DYNAMIC_STRIPE_CATALOG_IMPLEMENTATION_PROMPT.md`. End-to-end completion requires the Relay implementation to emit the canonical fixture shape.

## Objective

Remove hardcoded Commercial amounts and locally manufactured packages from macOS. Settings renders only provider-neutral offers returned by Ditch Relay.

The boundary remains:

```text
The Ditch macOS
→ Community ditchd
→ Ditch Relay
→ Stripe
```

Never add Stripe SDKs, direct Stripe calls, Stripe keys, Price IDs, Product IDs, Customer IDs, or provider-specific UI fields.

Do not redesign Community/Commercial composition, Remote Control, signed Commercial delivery, the one-runtime architecture, or local/SSH behavior.

## Current code to refactor

Inspect before editing. At minimum trace:

```text
crates/ditch_upgrade/src/lib.rs
crates/ditch_protocol/src/lib.rs
crates/ditchd/src/main.rs
apps/macos/lib/main.dart
apps/macos/test/widget_test.dart
commercial equivalents only where the private composition duplicates this client/runtime contract
```

Remove active production assumptions including:

```text
PricingOffer.price_eur_cents
locally fixed PricingInterval choices
fallback_offers()
CommercialOffers returning fallback_offers() without Relay
hardcoded offer-ID-to-checkout-route matching
UI assumptions about euros, whole-euro prices, month/lifetime-only filtering
tests asserting local €5 and €250 fixtures as product truth
```

## Provider-neutral offer model

Consume the Relay's Ditch-domain response. The model contains no Stripe identifiers or provider name.

Support at least:

```text
opaque offer_id
offer kind: commercial_monthly, commercial_lifetime, lifetime_extra_pair
title and description
currency
normal/base unit amount in exact minor-unit/decimal form
billing type: recurring or one_time
recurring interval and interval count
optional paid introductory amount
optional introductory duration and unit
purchase_action: acquire, add_capacity, or upgrade
eligible and sanitized ineligible_reason
fixed display-only Ditch entitlement summary
catalog freshness/stale indicator
```

Relay is authoritative. Do not infer entitlements from amount, title, description, or interval.

## Runtime flow

Change `CommercialOffers` so Community `ditchd` gets offers from `HttpUpgradeBackend`/Relay rather than `fallback_offers()`.

Add or refactor behavior equivalent to:

```text
commercial_offers(installation_identity)
create_checkout(installation_identity, opaque_offer_id)
```

Send only the opaque `offer_id`. Do not select Relay routes by matching local strings such as `commercial-monthly`.

Keep checkout, entitlement polling, license redemption, billing management, Commercial download, and artifact verification provider-neutral.

On catalog failure:

```text
do not show fallback prices
do not show zero-valued placeholders
show “Pricing is temporarily unavailable” and Retry
preserve all local and SSH Community functionality
```

## Money and billing formatting

Render returned ISO currencies and exact minor-unit/decimal amounts. Do not assume EUR, two decimal places, whole euros, or `/month` for every recurring offer.

Use locale-aware Flutter/macOS currency formatting. Avoid floating-point money calculations.

Support examples including:

```text
€15/month
€250 once
€7.50/month for 3 months, then €15/month
future Ditch-approved annual intervals without new formatting architecture
```

## Discounted introductory-price UI

The approved Monthly introductory offer is:

```text
€7.50/month for the first 3 months
then €15/month
```

This is paid introductory/trial pricing, not a free trial.

When Relay supplies a valid introductory amount below the base amount, render:

```text
the normal €15/month visibly struck through
the new €7.50/month emphasized
“for the first 3 months” adjacent or directly beneath
“Then €15/month” visible before checkout
```

Use `TextDecoration.lineThrough` only on the original/base price. Do not rely on color or strikethrough alone. Supply VoiceOver semantics equivalent to:

```text
Introductory price: 7 euros and 50 cents per month for 3 months. Then 15 euros per month.
```

Do not calculate a discount percentage locally unless Relay explicitly returns one. Do not infer an introductory period from two unrelated offers.

## Settings before and after subscription

Before active Commercial entitlement:

```text
show eligible Relay acquisition offers
show the introductory offer only when Relay says eligible
show standard Monthly and Lifetime when available
do not show Lifetime Extra Pair as a first-purchase plan
```

After active Commercial entitlement:

```text
remove ordinary plan/package acquisition cards
show current Commercial status and sanitized plan summary
show Remote Control/device state as appropriate for the edition
show Manage Billing when available
do not advertise the plan already owned
```

There are currently no plan-to-plan upgrades, so an active subscriber sees no plan-selection cards.

Prepare future upgrade rendering without local policy:

```text
show only eligible Relay offers with purchase_action=upgrade
place them in a restrained “Available upgrade” section
never compare prices or names locally to decide upgrade relationships
```

Lifetime Extra Pair is capacity management, not acquisition. When Relay returns `purchase_action=add_capacity` for an eligible Lifetime owner, expose it separately under “Add capacity” or device capacity. Do not keep the full acquisition pricing page visible for it.

Expired subscription behavior remains:

```text
Commercial subscription expired.
Local and SSH Ditch continue to work.
[Renew]
```

Relay decides renewal eligibility. Renewal is not a first-time introductory purchase.

## Loading and transitions

Handle:

```text
loading entitlement and catalog coherently
catalog available
catalog unavailable with Retry
checkout opening
Finishing upgrade… while Relay entitlement is pending
active entitlement with acquisition cards removed
expired entitlement with renewal
eligible future upgrade
eligible Lifetime capacity add-on
```

Browser success is never payment proof. Only Relay entitlement can switch the UI to subscribed.

Avoid plan-card flicker after subscription. If entitlement is already active, do not briefly render acquisition cards while entitlement loading is incomplete.

## Community boundary

Community may contain provider-neutral offer metadata, upgrade discovery, checkout bootstrap, entitlement summary, and Commercial installer bootstrap.

It must not gain Stripe implementation, Remote Control implementation, Relay WebSocket control, mobile pairing/commands, or proprietary enforcement.

Update `scripts/check-community-leakage` only for a false positive against legitimate provider-neutral bootstrap code. Do not weaken Remote Control checks.

## Tests

Use a fake Relay/upgrade backend. Never contact Stripe.

Cover at least:

```text
no hardcoded production prices remain
standard €15 Monthly renders from Relay data
paid intro renders €15 struck through and €7.50 emphasized
intro says first 3 months and then €15/month
intro is never called free
currency formatting retains cents
one-time Lifetime formatting
multiple active Monthly offers render independently
ineligible intro is absent or disabled per Relay contract
opaque offer ID passes unchanged through UI and ditchd
checkout URL opens in the system browser
pending checkout waits for Relay entitlement
failed checkout never claims activation
active subscription removes acquisition plan/package cards
active subscription retains Manage Billing
active subscriber sees no upgrade cards when Relay returns none
future eligible upgrade appears only in upgrade section
Lifetime Extra Pair appears under Add capacity
expired subscription preserves local/SSH behavior and offers renewal
catalog failure shows Retry with no fabricated price
Community leakage gate passes
Flutter analyze/test and Community Rust tests pass
```

Include a widget test inspecting the original-price `Text` style for `TextDecoration.lineThrough`; checking that both strings exist is insufficient.

## Definition of done

```text
1. No active Settings amount is hardcoded in macOS/Rust production code.
2. Community ditchd gets provider-neutral offers from Relay.
3. No Stripe identifier or SDK enters macOS.
4. Every valid Relay-returned Price renders without an app release.
5. Paid intro pricing shows struck base price, discounted price, duration, and later price.
6. The UI never calls a paid intro period free.
7. Acquisition cards disappear after activation.
8. Subscribers see status and Manage Billing instead of repeated upsell.
9. Future upgrades and Lifetime capacity add-ons have separate Relay-driven surfaces.
10. Community local and SSH behavior is unchanged.
11. Relevant Rust, Flutter, and leakage tests pass.
```

## Final report

Report only client-side changes: offer/protocol models, Relay flow, removed hardcoding, money formatting, introductory rendering, post-subscription visibility, upgrade/add-capacity behavior, tests, and any outstanding Relay dependency.
