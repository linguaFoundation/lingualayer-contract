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
    let contract_id = env.register_contract(None, RoyaltySplitter);
    let client = RoyaltySplitterClient::new(&env, &contract_id);

    client.initialize(&admin);

    // We need a mock token.
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
    let token_client = token::StellarAssetClient::new(&env, &token_contract);
    let token_std_client = token::Client::new(&env, &token_contract);

    let treasury = Address::generate(&env);
    let contributor1 = Address::generate(&env);
    let contributor2 = Address::generate(&env);

    let mut contributors = Vec::new(&env);
    contributors.push_back((contributor1.clone(), 6000));
    contributors.push_back((contributor2.clone(), 4000));

    let config = SplitConfig {
        dataset_id: String::from_str(&env, "ds_1"),
        token: token_contract.clone(),
        treasury: treasury.clone(),
        contributors,
    };

    client.register_split(&config);

    // Mint tokens to the contract so it can distribute
    token_client.mint(&contract_id, &100_000);

    client.distribute(&String::from_str(&env, "ds_1"), &100_000);

    // Treasury gets 5% = 5,000
    assert_eq!(token_std_client.balance(&treasury), 5_000);

    // Remaining 95,000 is split 60% / 40%
    assert_eq!(token_std_client.balance(&contributor1), 57_000);
    assert_eq!(token_std_client.balance(&contributor2), 38_000);
}

#[test]
fn test_treasury_fee_deducted() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, RoyaltySplitter);
    let client = RoyaltySplitterClient::new(&env, &contract_id);

    client.initialize(&admin);

    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
    let token_client = token::StellarAssetClient::new(&env, &token_contract);
    let token_std_client = token::Client::new(&env, &token_contract);

    let treasury = Address::generate(&env);
    let contributor1 = Address::generate(&env);

    let mut contributors = Vec::new(&env);
    contributors.push_back((contributor1.clone(), 10000));

    let config = SplitConfig {
        dataset_id: String::from_str(&env, "ds_2"),
        token: token_contract.clone(),
        treasury: treasury.clone(),
        contributors,
    };

    client.register_split(&config);
    token_client.mint(&contract_id, &200_000);

    client.distribute(&String::from_str(&env, "ds_2"), &200_000);

    assert_eq!(token_std_client.balance(&treasury), 10_000);
}
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
fn test_distribute_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, RoyaltySplitter);
    let client = RoyaltySplitterClient::new(&env, &contract_id);

    client.initialize(&admin);

    let token_contract = Address::generate(&env);
    let treasury = Address::generate(&env);
    let contributor1 = Address::generate(&env);

    let mut contributors = Vec::new(&env);
    contributors.push_back((contributor1.clone(), 10000));

    let config = SplitConfig {
        dataset_id: String::from_str(&env, "ds_3"),
        token: token_contract,
        treasury,
        contributors,
    };

    client.register_split(&config);
    client.distribute(&String::from_str(&env, "ds_3"), &0);
}
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

    token_id
}

#[test]
fn test_distribute_records_quality_tier_from_oracle() {
    let env = Env::default();
    env.mock_all_auths();
    let (splitter, oracle, _admin, splitter_id) = setup(&env);
    let dataset_id = String::from_str(&env, "ds_gold");
    let contributor = Address::generate(&env);
    setup_split(&env, &splitter, &splitter_id, &dataset_id, &contributor);

    let curator = Address::generate(&env);
    oracle.register_curator(&curator);
    oracle.attest_quality(&curator, &dataset_id, &75, &BytesN::from_array(&env, &[1u8; 32])); // Gold: 70-84

    splitter.distribute(&dataset_id, &100_000);

    let count = splitter.payout_count();
    assert_eq!(count, 1);
    let record = splitter.get_payout(&count);
    assert_eq!(record.quality_tier, String::from_str(&env, "Gold"));
}

#[test]
fn test_distribute_defaults_to_unrated_when_never_attested() {
    let env = Env::default();
    env.mock_all_auths();
    let (splitter, _oracle, _admin, splitter_id) = setup(&env);
    let dataset_id = String::from_str(&env, "ds_never_attested");
    let contributor = Address::generate(&env);
    setup_split(&env, &splitter, &splitter_id, &dataset_id, &contributor);

    splitter.distribute(&dataset_id, &100_000);

    let count = splitter.payout_count();
    assert_eq!(count, 1);
    let record = splitter.get_payout(&count);
    assert_eq!(record.quality_tier, String::from_str(&env, "Unrated"));
}
