#![cfg(test)]

use super::*;
use soroban_sdk::{Env, BytesN, testutils::Address as _, Address, String};

#[test]
#[should_panic(expected = "metadata hash cannot be zero")]
fn test_post_commission_zero_hash_panics() {
    let env = Env::default();
    let commissioner = Address::generate(&env);
    let bounty_token = Address::generate(&env);
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);
    
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let zero_hash = BytesN::from_array(&env, &[0u8; 32]);
    
    env.mock_all_auths();

    client.post_commission(
        &commissioner,
        &String::from_str(&env, "en"),
        &zero_hash,
        &bounty_token,
        &1000,
        &100,
        &3600,
        &9999999, // deadline
    );
}

#[test]
fn test_post_commission_valid_hash_succeeds() {
    let env = Env::default();
    let commissioner = Address::generate(&env);
    
    // We need a real token contract to mock the token transfer
    let bounty_token = env.register_stellar_asset_contract(commissioner.clone());
    
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);
    
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 1; // non-zero
    let valid_hash = BytesN::from_array(&env, &hash_bytes);
    
    env.mock_all_auths();

    let id = client.post_commission(
        &commissioner,
        &String::from_str(&env, "en"),
        &valid_hash,
        &bounty_token,
        &1000,
        &100,
        &3600,
        &9999999,
    );
    
    assert_eq!(id, String::from_str(&env, "com_1"));
}
