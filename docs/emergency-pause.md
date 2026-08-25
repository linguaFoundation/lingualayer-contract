# Emergency pause

Every contract in the workspace can be frozen by its admin. The point is
incident response: if a critical vulnerability is found before or during
testnet, the damage window becomes however long it takes to land one
transaction, rather than however long it takes to write, review, and deploy a
fix.

## Interface

Identical in all five contracts, so an operator running the freeze during an
incident does not have to remember per-contract differences:

| Function | Auth | Returns |
| --- | --- | --- |
| `pause()` | admin | `Err(Error::AlreadyPaused)` if already frozen |
| `unpause()` | admin | `Err(Error::NotPaused)` if not frozen |
| `is_paused() -> bool` | none | A read, so it answers while paused |

Both transitions emit an event carrying the admin address and the ledger
timestamp: `("pause", "paused")` and `("pause", "unpaused")`.

Frozen writes return `Err(Error::ContractPaused)`, following the typed-error
convention the contracts adopted when panics were refactored into
`#[contracterror]` variants. `slash_curator` is the one exception: it still
returns `()` and signals failure by panicking, so its pause check panics too
rather than introducing a second error style inside one function.

## What the freeze covers

State-mutating entry points across all five contracts:

- **dataset-registry** — `register_dataset`, `update_metadata`, `flag_dataset`,
  `reinstate_dataset`, `deprecate_dataset`
- **license-router** — `issue_license`, `revoke_license`, `set_oracle`
- **royalty-splitter** — `register_split`, `distribute`, `set_oracle`
- **quality-oracle** — `register_curator`, `attest_quality`, `slash_curator`
- **data-commission** — `post_commission`, `fulfil_commission`,
  `cancel_commission`, `set_milestones`, `release_milestone`, `set_arbiter`,
  `raise_dispute`, `resolve_dispute`

Reads are never blocked. Integrators and the front end have to keep answering
questions about existing state during an incident, and a read cannot make the
problem worse.

The check runs *before* authorization. A paused contract rejects the call
whoever is making it, so there is no reason to do the more expensive auth work
first, and no signature is spent on a call that was never going to land.

## Three deliberate exclusions

These are the decisions worth arguing about, so they are stated rather than
buried.

**TTL renewals stay callable while paused** — `renew_dataset_ttl`,
`renew_reputation_ttl`, `renew_license_ttl`, `renew_split_ttl`,
`renew_payout_ttl`, `renew_quality_ttl`.

They are technically writes, so a strict reading of "freeze all writes" would
include them. But they mutate nothing semantic: they only push back the expiry
on an entry that already exists. Freezing them means that a pause lasting
longer than a TTL window silently archives datasets, licences, and quality
records — the pause would destroy exactly the state it was invoked to protect.
An incident that lasts weeks is precisely when renewals matter most.

**Admin handoff stays callable while paused** — `propose_admin` /
`accept_admin`.

If the reason for the pause is a compromised admin key, rotating that key is
the first thing an operator needs to do, and it cannot require unpausing first.
The handoff is already two-step and needs the incoming admin's own signature,
so leaving it live does not widen the attack surface.

**`upgrade` stays callable while paused.**

Deploying the fixed WASM is the reason the pause exists. Guarding `upgrade`
would mean an operator has to unpause — reopening the vulnerability — in order
to close it. It is admin-only already.

All three are one-line changes if maintainers would rather have the strict
reading. `initialize` is also unguarded, since there is no admin to authorize a
pause before it runs.

## Tests

Each contract has a `pause_test` module covering: a fresh contract is unpaused;
pause/unpause round-trips; a write is rejected while paused; the same write
succeeds after unpausing; reads still answer while paused; a non-admin cannot
pause; and double-pause / double-unpause are rejected.
