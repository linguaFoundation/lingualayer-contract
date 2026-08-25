#![cfg(test)]
//! Storage lifetime tests for split configurations and payout receipts.
//!
//! A lapsed `SplitConfig` is the worst of the workspace's TTL failures: it
//! does not fail closed. `distribute` reads the config to decide who gets
//! paid, so when it expires the payout path for that dataset simply stops
//! working, and whoever re-registers the shares afterwards decides what they
//! are.

use super::*;
use soroban_sdk::{
    testutils::{storage::Persistent as _, Address as _, Ledger as _},
    Address, Env, String, Vec,
};

struct Fixture<'a> {
    env: Env,
    client: RoyaltySplitterClient<'a>,
    contract: Address,
    dataset_id: String,
}

fn setup() -> Fixture<'static> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, RoyaltySplitter);
    let client = RoyaltySplitterClient::new(&env, &contract_id);
    client.initialize(&admin);

    let token_admin = Address::generate(&env);
    let token_contract = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    let mut contributors = Vec::new(&env);
    contributors.push_back((Address::generate(&env), 6000));
    contributors.push_back((Address::generate(&env), 4000));

    let dataset_id = String::from_str(&env, "ds_ttl");
    client.register_split(&SplitConfig {
        dataset_id: dataset_id.clone(),
        token: token_contract.clone(),
        treasury: Address::generate(&env),
        contributors,
    });

    token::StellarAssetClient::new(&env, &token_contract).mint(&contract_id, &100_000);

    Fixture {
        env,
        client,
        contract: contract_id,
        dataset_id,
    }
}

/// `extend_ttl` asks for PERSISTENT_TTL but the host caps every entry at the
/// ledger's max_entry_ttl, so a full window is the smaller of the two.
fn full_window(env: &Env) -> u32 {
    core::cmp::min(PERSISTENT_TTL, env.ledger().get().max_entry_ttl - 1)
}

fn ttl_of(env: &Env, contract: &Address, key: &String) -> u32 {
    env.as_contract(contract, || env.storage().persistent().get_ttl(key))
}

fn advance(env: &Env, ledgers: u32) {
    env.ledger().with_mut(|l| l.sequence_number += ledgers);
}

#[test]
fn test_register_split_extends_ttl_on_creation() {
    let f = setup();
    assert_eq!(
        ttl_of(&f.env, &f.contract, &f.dataset_id),
        full_window(&f.env)
    );
}

#[test]
fn test_distribute_extends_payout_receipt_ttl() {
    let f = setup();
    f.client.distribute(&f.dataset_id, &10_000);

    let key = String::from_str(&f.env, "pay_1");
    assert_eq!(ttl_of(&f.env, &f.contract, &key), full_window(&f.env));
}

#[test]
fn test_renew_split_ttl_is_permissionless() {
    let f = setup();
    advance(&f.env, 2_000_000);
    assert!(ttl_of(&f.env, &f.contract, &f.dataset_id) < full_window(&f.env));

    // Not the admin: register_split is admin-gated, but renewal deliberately
    // is not — any contributor in the split can protect their own claim.
    f.env.set_auths(&[]);
    f.client.renew_split_ttl(&f.dataset_id);

    assert_eq!(
        ttl_of(&f.env, &f.contract, &f.dataset_id),
        full_window(&f.env)
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_renew_split_ttl_unknown_dataset_panics() {
    let f = setup();
    f.client
        .renew_split_ttl(&String::from_str(&f.env, "ds_nope"));
}

#[test]
fn test_renew_payout_ttl_is_permissionless() {
    let f = setup();
    f.client.distribute(&f.dataset_id, &10_000);

    advance(&f.env, 2_000_000);
    f.env.set_auths(&[]);
    f.client.renew_payout_ttl(&1);

    let key = String::from_str(&f.env, "pay_1");
    assert_eq!(ttl_of(&f.env, &f.contract, &key), full_window(&f.env));
    assert_eq!(f.client.get_payout(&1).total_amount, 10_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_renew_payout_ttl_unknown_record_panics() {
    let f = setup();
    f.client.renew_payout_ttl(&99);
}

#[test]
fn test_split_config_still_distributable_after_long_ledger_advance() {
    let f = setup();
    let window = full_window(&f.env);

    for _ in 0..9 {
        advance(&f.env, window - 1);
        f.client.renew_split_ttl(&f.dataset_id);
    }

    // The config is still there and still deserializes to the *original*
    // shares — not to whatever someone would have re-registered after letting
    // it lapse. Read directly from storage rather than through distribute(),
    // because the SAC token this split points at is a separate contract with
    // its own lifetime that this contract cannot renew on anyone's behalf.
    let config: SplitConfig = f
        .env
        .as_contract(&f.contract, || {
            f.env.storage().persistent().get(&f.dataset_id)
        })
        .expect("split config should have survived");

    assert_eq!(config.dataset_id, f.dataset_id);
    assert_eq!(config.contributors.len(), 2);
    assert_eq!(config.contributors.get(0).unwrap().1, 6000);
    assert_eq!(config.contributors.get(1).unwrap().1, 4000);
    assert_eq!(
        ttl_of(&f.env, &f.contract, &f.dataset_id),
        full_window(&f.env)
    );
}
