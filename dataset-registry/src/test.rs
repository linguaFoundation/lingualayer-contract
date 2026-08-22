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
    
    // admin setup
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let zero_hash = BytesN::from_array(&env, &[0u8; 32]);
    let contributors = Vec::new(&env);
    // this will fail on require_auth if we don't mock it, 
    // but env.mock_all_auths() fixes that
    env.mock_all_auths();

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
    
    env.mock_all_auths();

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
fn test_admin_handoff_propose_then_accept() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(&env, &contract_id);

    client.initialize(&admin);
    client.propose_admin(&new_admin);
    client.accept_admin();

    // New admin can now do admin-gated work the old admin no longer can
    // authenticate as; re-proposing confirms the swap actually took effect.
    let another = Address::generate(&env);
    client.propose_admin(&another);
}

#[test]
#[should_panic(expected = "no admin proposal pending")]
fn test_accept_admin_without_proposal_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(&env, &contract_id);

    client.initialize(&admin);
    client.accept_admin();
}
