#![cfg(test)]

use super::*;
use soroban_sdk::{Env, BytesN, testutils::Address as _, Address, String, Vec};

#[test]
#[should_panic(expected = "metadata hash cannot be zero")]
fn test_register_zero_hash_panics() {
    let env = Env::default();
    let owner = Address::generate(&env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(&env, &contract_id);

    // this will fail on require_auth if we don't mock it,
    // but env.mock_all_auths() fixes that — must come before any
    // call that itself requires auth, including initialize()
    env.mock_all_auths();

    // admin setup
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let zero_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contributors = Vec::new(&env);

    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Test Dataset"),
        &zero_hash,
        &contributors,
        &100,
        &3600,
        &None,
    );
}

#[test]
fn test_register_valid_hash_succeeds() {
    let env = Env::default();
    let owner = Address::generate(&env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(&env, &contract_id);

    env.mock_all_auths();

    // admin setup
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 1; // non-zero
    let valid_hash = BytesN::from_array(&env, &hash_bytes);

    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 10000,
    });

    let id = client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Test Dataset"),
        &valid_hash,
        &contributors,
        &100,
        &3600,
        &None,
    );
    
    assert_eq!(id, String::from_str(&env, "ds_1"));
}

#[test]
#[should_panic(expected = "dataset with this metadata hash is already registered")]
fn test_register_duplicate_hash_panics() {
    let env = Env::default();
    let owner = Address::generate(&env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(&env, &contract_id);

    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 9;
    let dup_hash = BytesN::from_array(&env, &hash_bytes);

    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 10000,
    });

    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "First"),
        &dup_hash,
        &contributors,
        &100,
        &3600,
        &None,
    );

    // Same metadata_hash, different owner/name/language — should still be
    // rejected, since the hash is what identifies the underlying dataset.
    let other_owner = Address::generate(&env);
    let mut other_contributors = Vec::new(&env);
    other_contributors.push_back(ContributorShare {
        address: other_owner.clone(),
        share_bps: 10000,
    });
    client.register_dataset(
        &other_owner,
        &String::from_str(&env, "fr"),
        &String::from_str(&env, "Second"),
        &dup_hash,
        &other_contributors,
        &200,
        &7200,
        &None,
    );
}

#[test]
fn test_dataset_id_for_hash_returns_id_after_registration_and_none_before() {
    let env = Env::default();
    let owner = Address::generate(&env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(&env, &contract_id);

    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 5;
    let hash = BytesN::from_array(&env, &hash_bytes);

    assert_eq!(client.dataset_id_for_hash(&hash), None);

    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 10000,
    });
    let id = client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Test Dataset"),
        &hash,
        &contributors,
        &100,
        &3600,
        &None,
    );

    assert_eq!(client.dataset_id_for_hash(&hash), Some(id));
}
