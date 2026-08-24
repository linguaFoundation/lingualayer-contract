#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, String, Vec,
};

struct Fixture<'a> {
    env: Env,
    client: RoyaltySplitterClient<'a>,
    token: TokenClient<'a>,
    contract: Address,
    treasury: Address,
}

/// Deploy the splitter alongside a SAC token, initialize an admin, and fund
/// the splitter so it has a balance to distribute from.
fn setup(funding: i128) -> Fixture<'static> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);

    let issuer = Address::generate(&env);
    let token_address = env.register_stellar_asset_contract(issuer.clone());

    let contract = env.register_contract(None, RoyaltySplitter);
    let client = RoyaltySplitterClient::new(&env, &contract);
    client.initialize(&admin);

    if funding > 0 {
        StellarAssetClient::new(&env, &token_address).mint(&contract, &funding);
    }

    let token = TokenClient::new(&env, &token_address);
    Fixture {
        env,
        client,
        token,
        contract,
        treasury,
    }
}

fn dataset(env: &Env) -> String {
    String::from_str(env, "ds_1")
}

impl Fixture<'_> {
    fn register(&self, contributors: Vec<(Address, u32)>) {
        self.client.register_split(&SplitConfig {
            dataset_id: dataset(&self.env),
            token: self.token.address.clone(),
            treasury: self.treasury.clone(),
            contributors,
        });
    }
}

fn shares(env: &Env, entries: &[(Address, u32)]) -> Vec<(Address, u32)> {
    let mut v = Vec::new(env);
    for (address, bps) in entries {
        v.push_back((address.clone(), *bps));
    }
    v
}

#[test]
fn test_distribute_pays_treasury_five_percent_and_splits_remainder() {
    let f = setup(1_000_000);
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    f.register(shares(
        &f.env,
        &[(alice.clone(), 7000), (bob.clone(), 3000)],
    ));

    f.client.distribute(&dataset(&f.env), &1_000_000);

    // 5% of 1,000,000 = 50,000; the remaining 950,000 splits 70/30.
    assert_eq!(f.token.balance(&f.treasury), 50_000);
    assert_eq!(f.token.balance(&alice), 665_000);
    assert_eq!(f.token.balance(&bob), 285_000);
    assert_eq!(f.token.balance(&f.contract), 0);
}

#[test]
fn test_contributor_payouts_sum_exactly_to_total_minus_fee_with_dust() {
    // 1000 splits three ways at 3333/3333/3334 bps. The floored per-share
    // payouts leave dust behind, which must still reach contributors.
    let f = setup(1_000);
    let a = Address::generate(&f.env);
    let b = Address::generate(&f.env);
    let c = Address::generate(&f.env);
    f.register(shares(
        &f.env,
        &[(a.clone(), 3333), (b.clone(), 3333), (c.clone(), 3334)],
    ));

    f.client.distribute(&dataset(&f.env), &1_000);

    let fee = f.token.balance(&f.treasury);
    assert_eq!(fee, 50); // 1000 * 500 / 10000

    let paid = f.token.balance(&a) + f.token.balance(&b) + f.token.balance(&c);
    assert_eq!(paid, 1_000 - fee);

    // Nothing is stranded in the contract.
    assert_eq!(f.token.balance(&f.contract), 0);

    // The reconciling stroop lands on the largest shareholder.
    assert_eq!(f.token.balance(&a), 316);
    assert_eq!(f.token.balance(&b), 316);
    assert_eq!(f.token.balance(&c), 318);
}

#[test]
fn test_repeated_distributions_never_strand_dust() {
    let f = setup(3_000);
    let a = Address::generate(&f.env);
    let b = Address::generate(&f.env);
    let c = Address::generate(&f.env);
    f.register(shares(
        &f.env,
        &[(a.clone(), 3333), (b.clone(), 3333), (c.clone(), 3334)],
    ));

    for _ in 0..3 {
        f.client.distribute(&dataset(&f.env), &1_000);
    }

    assert_eq!(f.token.balance(&f.contract), 0);
    assert_eq!(f.client.payout_count(), 3);
}

#[test]
fn test_single_contributor_receives_entire_distributable() {
    let f = setup(10_000);
    let solo = Address::generate(&f.env);
    f.register(shares(&f.env, &[(solo.clone(), 10000)]));

    f.client.distribute(&dataset(&f.env), &10_000);

    assert_eq!(f.token.balance(&f.treasury), 500);
    assert_eq!(f.token.balance(&solo), 9_500);
    assert_eq!(f.token.balance(&f.contract), 0);
}

#[test]
fn test_payout_record_is_persisted_with_ledger() {
    let f = setup(1_000_000);
    let alice = Address::generate(&f.env);
    f.register(shares(&f.env, &[(alice.clone(), 10000)]));

    assert_eq!(f.client.payout_count(), 0);
    f.client.distribute(&dataset(&f.env), &400_000);
    assert_eq!(f.client.payout_count(), 1);

    let record = f.client.get_payout(&1);
    assert_eq!(record.dataset_id, dataset(&f.env));
    assert_eq!(record.total_amount, 400_000);
    assert_eq!(record.tx_count, 1);
    assert_eq!(record.ledger, f.env.ledger().sequence());
}

#[test]
#[should_panic(expected = "amount must be positive")]
fn test_zero_amount_distribute_panics() {
    let f = setup(1_000);
    let alice = Address::generate(&f.env);
    f.register(shares(&f.env, &[(alice, 10000)]));

    f.client.distribute(&dataset(&f.env), &0);
}

#[test]
#[should_panic(expected = "amount must be positive")]
fn test_negative_amount_distribute_panics() {
    let f = setup(1_000);
    let alice = Address::generate(&f.env);
    f.register(shares(&f.env, &[(alice, 10000)]));

    f.client.distribute(&dataset(&f.env), &-1);
}

#[test]
#[should_panic(expected = "insufficient contract balance for distribution")]
fn test_distribute_beyond_balance_panics_before_transferring() {
    let f = setup(100);
    let alice = Address::generate(&f.env);
    f.register(shares(&f.env, &[(alice, 10000)]));

    f.client.distribute(&dataset(&f.env), &1_000);
}

#[test]
#[should_panic(expected = "split config not found")]
fn test_distribute_without_config_panics() {
    let f = setup(1_000);
    f.client.distribute(&dataset(&f.env), &1_000);
}

#[test]
#[should_panic(expected = "contributor shares must sum to 10000 bps")]
fn test_register_split_with_bad_shares_panics() {
    let f = setup(0);
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    f.register(shares(&f.env, &[(alice, 6000), (bob, 5000)]));
}

#[test]
fn test_register_split_is_readable() {
    let f = setup(0);
    let alice = Address::generate(&f.env);
    f.register(shares(&f.env, &[(alice.clone(), 10000)]));

    let config = f.client.get_split(&dataset(&f.env));
    assert_eq!(config.treasury, f.treasury);
    assert_eq!(config.contributors.len(), 1);
}

#[test]
#[should_panic(expected = "already initialized")]
fn test_double_initialize_panics() {
    let f = setup(0);
    f.client.initialize(&Address::generate(&f.env));
}

#[test]
#[should_panic(expected = "not initialized")]
fn test_register_split_before_initialize_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register_contract(None, RoyaltySplitter);
    let client = RoyaltySplitterClient::new(&env, &contract);
    let issuer = Address::generate(&env);
    let token_address = env.register_stellar_asset_contract(issuer);
    let alice = Address::generate(&env);

    client.register_split(&SplitConfig {
        dataset_id: String::from_str(&env, "ds_1"),
        token: token_address,
        treasury: Address::generate(&env),
        contributors: shares(&env, &[(alice, 10000)]),
    });
}

#[test]
fn test_admin_handoff_propose_then_accept() {
    let f = setup(0);
    let new_admin = Address::generate(&f.env);

    f.client.propose_admin(&new_admin);
    f.client.accept_admin();

    let another = Address::generate(&f.env);
    f.client.propose_admin(&another);
}

#[test]
#[should_panic(expected = "no admin proposal pending")]
fn test_accept_admin_without_proposal_panics() {
    let f = setup(0);
    f.client.accept_admin();
}
