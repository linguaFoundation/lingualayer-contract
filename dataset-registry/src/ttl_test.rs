#![cfg(test)]
//! Storage lifetime tests: every persistent entry this contract writes must
//! come out of the write with a full TTL window, and must be renewable by
//! anyone afterwards without a mutating call.

use super::*;
use soroban_sdk::{
    testutils::{storage::Persistent as _, Address as _, Ledger as _},
    Address, BytesN, Env, String, Vec,
};

fn setup(env: &Env) -> (DatasetRegistryClient<'static>, Address) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(env, &contract_id);
    client.initialize(&admin);
    (client, admin)
}

fn hash_of(env: &Env, byte: u8) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = byte;
    BytesN::from_array(env, &bytes)
}

fn register(
    env: &Env,
    client: &DatasetRegistryClient,
    owner: &Address,
    hash: &BytesN<32>,
) -> String {
    let mut contributors = Vec::new(env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 10000,
    });
    client.register_dataset(
        owner,
        &String::from_str(env, "yo"),
        &String::from_str(env, "TTL Dataset"),
        hash,
        &contributors,
        &100,
        &3600,
        &None,
    )
}

/// The TTL a freshly written entry actually ends up with. `extend_ttl` asks
/// for PERSISTENT_TTL, but the host caps any entry at the ledger's
/// max_entry_ttl, so a full window is the smaller of the two.
fn full_window(env: &Env) -> u32 {
    core::cmp::min(PERSISTENT_TTL, env.ledger().get().max_entry_ttl - 1)
}

/// TTL of a persistent key, read from inside the contract's own storage.
fn ttl_of(env: &Env, contract: &Address, key: &String) -> u32 {
    env.as_contract(contract, || env.storage().persistent().get_ttl(key))
}

/// Advance the ledger far enough that an entry left on the default minimum
/// TTL would be long gone.
fn advance(env: &Env, ledgers: u32) {
    env.ledger().with_mut(|l| l.sequence_number += ledgers);
}

#[test]
fn test_register_dataset_extends_ttl_on_creation() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let hash = hash_of(&env, 1);
    let id = register(&env, &client, &owner, &hash);

    let hash_key = String::from_str(&env, &alloc::format!("hash_{:?}", hash));
    // Both the record and its hash index leave registration with a full window.
    assert_eq!(ttl_of(&env, &client.address, &id), full_window(&env));
    assert_eq!(ttl_of(&env, &client.address, &hash_key), full_window(&env));
}

#[test]
fn test_update_metadata_renews_ttl() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let id = register(&env, &client, &owner, &hash_of(&env, 1));

    advance(&env, 100_000);
    // Decayed by the ledgers that have passed.
    assert!(ttl_of(&env, &client.address, &id) < full_window(&env));

    client.update_metadata(&id, &hash_of(&env, 2));
    assert_eq!(ttl_of(&env, &client.address, &id), full_window(&env));
}

#[test]
fn test_renew_dataset_ttl_is_permissionless() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let id = register(&env, &client, &owner, &hash_of(&env, 1));

    advance(&env, 1_000_000);
    let decayed = ttl_of(&env, &client.address, &id);
    assert!(decayed < full_window(&env));

    // No require_auth on this path at all: with every mock auth cleared, a
    // caller who is neither the owner nor the admin still gets through.
    env.set_auths(&[]);
    client.renew_dataset_ttl(&id);

    assert_eq!(ttl_of(&env, &client.address, &id), full_window(&env));
}

#[test]
fn test_renew_dataset_ttl_also_renews_hash_index() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let hash = hash_of(&env, 9);
    let id = register(&env, &client, &owner, &hash);
    let hash_key = String::from_str(&env, &alloc::format!("hash_{:?}", hash));

    advance(&env, 1_000_000);
    client.renew_dataset_ttl(&id);

    // The index must not be allowed to lapse independently of the record —
    // dataset_id_for_hash would start lying about a dataset that still exists.
    assert_eq!(ttl_of(&env, &client.address, &hash_key), full_window(&env));
    assert_eq!(client.dataset_id_for_hash(&hash), Some(id));
}

#[test]
#[should_panic(expected = "dataset not found")]
fn test_renew_dataset_ttl_unknown_id_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    client.renew_dataset_ttl(&String::from_str(&env, "ds_nope"));
}

#[test]
fn test_dataset_readable_after_repeated_renewal_across_long_ledger_advance() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let hash = hash_of(&env, 3);
    let id = register(&env, &client, &owner, &hash);

    // Ten full TTL windows' worth of ledgers, renewed before each window
    // lapses the way a community keeper would. The renewal interval has to
    // stay inside the window itself — a keeper who waits longer than one
    // window has already lost the record, which is the whole reason this
    // entry point has to exist rather than relying on incidental writes.
    let window = full_window(&env);
    for _ in 0..10 {
        advance(&env, window - 1);
        client.renew_dataset_ttl(&id);
    }

    let ds = client.get_dataset(&id);
    assert_eq!(ds.id, id);
    assert_eq!(ds.owner, owner);
    assert_eq!(ds.state, DatasetState::Active);
    assert_eq!(client.dataset_id_for_hash(&hash), Some(id));
}

#[test]
fn test_renew_reputation_ttl_is_permissionless() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    register(&env, &client, &owner, &hash_of(&env, 1));

    let rep_key = String::from_str(&env, &alloc::format!("rep_{:?}", owner));
    assert_eq!(ttl_of(&env, &client.address, &rep_key), full_window(&env));

    advance(&env, 2_000_000);
    env.set_auths(&[]);
    client.renew_reputation_ttl(&owner);

    assert_eq!(ttl_of(&env, &client.address, &rep_key), full_window(&env));
    assert_eq!(client.get_reputation(&owner).datasets_registered, 1);
}

#[test]
#[should_panic(expected = "no reputation data")]
fn test_renew_reputation_ttl_unknown_address_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    client.renew_reputation_ttl(&Address::generate(&env));
}
