# Contract Interaction Patterns

This documents how the five Soroban contracts in this workspace relate to
each other — what each owns, how a dataset moves through them end to end,
and (importantly) how little they actually talk to each other on-chain.
See the root [README](README.md) for the wider repo layout and deployed
addresses.

## The five contracts, one line each

| Contract | Owns | Key entity |
|---|---|---|
| `data-commission` | Bounty escrow for commissioned datasets | `Commission` |
| `dataset-registry` | Dataset metadata, contributor shares, reputation | `Dataset` |
| `quality-oracle` | Curator-staked quality attestations | `DatasetQuality` |
| `license-router` | Usage licenses by region/model class | `License` |
| `royalty-splitter` | Splitting license revenue to contributors | `SplitConfig` / `PayoutRecord` |

## The core fact: these contracts don't call each other

There is exactly one form of cross-contract invocation anywhere in this
workspace: `token::Client` calls into the USDC Stellar Asset Contract (in
`data-commission` and `royalty-splitter`, to move escrowed/license funds).
**None of the five contracts above invoke one another.** Search for
yourself — there's no `invoke_contract` or `<Other>Client::new(&env, ...)`
call between them anywhere in `src/`.

They're linked only by convention: they share string identifiers
(`dataset_id`, `commission_id`) as foreign keys, and it's up to whatever
calls them — today, a human/script issuing transactions directly, since
there's no on-chain orchestrator — to read state from one contract and
pass the relevant piece into the next. `license-router::initialize` even
accepts and stores a `registry_contract: Address`, which reads like the
seam for an on-chain read-through into `dataset-registry`, but nothing in
`issue_license` currently uses it — it's stored and otherwise inert.

This matters for anyone extending these contracts: **adding a real
cross-contract call is a bigger, riskier change than it looks** (auth
context propagation, the callee's own panics becoming your panics, WASM
call-depth/budget cost) — worth flagging explicitly in a PR rather than
adding quietly as a one-line `Client::new(...).some_fn(...)`.

## End-to-end lifecycle (as the ID conventions imply it)

```
 1. DataCommission::post_commission
        AI company escrows a USDC bounty for a language/sample-count spec.
        → commission_id (e.g. "com_7")

 2. (off-chain) a contributor produces the dataset

 3. DatasetRegistry::register_dataset
        Contributor registers the delivered dataset, optionally passing
        the commission_id from step 1 in Dataset.commission_id — the only
        place that field is read or written; nothing enforces the link
        beyond "the caller happened to pass a matching string".
        → dataset_id (e.g. "ds_12")

 4. DataCommission::fulfil_commission (or the milestone variants)
        Admin verifies delivery against the commission's requirements and
        releases escrow to the fulfiller. Independent of step 3 — you can
        fulfil a commission without ever registering a dataset for it, or
        vice versa.

 5. QualityOracle::register_curator + attest_quality (repeated, by
    different staked curators, over the dataset_id from step 3)
        Produces a running DatasetQuality — average_score, tier, and (via
        QualityOracle::royalty_multiplier_bps) a suggested multiplier.
        Nothing downstream reads this automatically — see below.

 6. LicenseRouter::issue_license (by a licensee, against dataset_id)
        Records that a license was issued and what fee_paid_stroops was
        declared. Does not itself move any tokens — there's no
        token::Client transfer in issue_license.

 7. RoyaltySplitter::register_split (dataset_id → SplitConfig)
        Someone — off-chain, reading DatasetRegistry's ContributorShare
        list — submits the payout shares for this dataset. This is the
        one point where dataset-registry's data conceptually needs to
        reach royalty-splitter, and today that trip is made entirely
        outside the chain.

 8. RoyaltySplitter::distribute(dataset_id, total_amount)
        Pulls total_amount via token::Client, takes a 5% treasury cut,
        splits the rest per the registered SplitConfig. total_amount is
        caller-supplied — QualityOracle::royalty_multiplier_bps is never
        read here, so today the quality tier does not automatically
        affect a payout on-chain. If a licensee's fee is meant to scale
        with quality, that scaling has to happen off-chain before calling
        distribute (or become a future on-chain read from QualityOracle).
```

## Storage conventions shared across contracts

All five contracts follow the same two patterns, which is worth knowing
before adding a new one:

- **Composite string keys.** Persistent storage keys are built with
  `format!("{prefix}_{:?}", id_or_address)` (e.g. `agg_{dataset_id}` in
  `quality-oracle`, `hash_{metadata_hash}` in `dataset-registry`,
  `cur_{curator}` for curator registration). There's no `Map`/index type
  in use anywhere — every "lookup by X" is a derived key, computed
  identically wherever it's read or written. If you add a new derived
  key, keep the `{prefix}_{...}` format so it doesn't collide with an
  existing one for a different entity.
- **`extend_ttl` on every persistent write.** New or updated persistent
  entries call `env.storage().persistent().extend_ttl(&key, 7_776_000,
  7_776_000)` right after `.set(...)` — roughly a 90-day TTL window,
  consistently applied. `data-commission` and `quality-oracle` (see #21)
  also expose a standalone, permissionless `renew_*_ttl` entry point so a
  record's lifetime doesn't depend on someone happening to trigger a
  mutating call before it lapses.

## Where this leaves a contributor

If you're implementing a feature that spans two of these contracts, the
honest options today are: (a) keep doing what steps 3, 6, and 7 above do
— pass the relevant ID as a plain argument and trust the caller to have
gotten it from the right place, or (b) add a real cross-contract call,
which is new territory for this codebase and should be scoped and
reviewed as such, not folded into an unrelated change.
