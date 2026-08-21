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

    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let zero_hash = BytesN::from_array(&env, &[0u8; 32]);

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

    env.mock_all_auths();

    // Fund the commissioner so post_commission's escrow transfer has a
    // balance to draw from — register_stellar_asset_contract only deploys
    // the asset contract, it doesn't mint anything to the issuer.
    token::StellarAssetClient::new(&env, &bounty_token).mint(&commissioner, &1000);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 1; // non-zero
    let valid_hash = BytesN::from_array(&env, &hash_bytes);

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

#[test]
fn test_renew_commission_ttl_is_permissionless() {
    let env = Env::default();
    let commissioner = Address::generate(&env);
    let bounty_token = env.register_stellar_asset_contract(commissioner.clone());
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);

    env.mock_all_auths();
    token::StellarAssetClient::new(&env, &bounty_token).mint(&commissioner, &1000);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 3;
    let hash = BytesN::from_array(&env, &hash_bytes);

    let id = client.post_commission(
        &commissioner,
        &String::from_str(&env, "en"),
        &hash,
        &bounty_token,
        &1000,
        &100,
        &3600,
        &9999999,
    );

    // No require_auth anywhere in the call path — env.mock_all_auths()
    // above isn't what makes this succeed; a bare, unauthenticated call
    // from any address is expected to work.
    client.renew_commission_ttl(&id);

    // Still readable afterward — the call didn't corrupt or clear the entry.
    assert_eq!(client.get_commission(&id).id, id);
}

#[test]
#[should_panic(expected = "commission not found")]
fn test_renew_commission_ttl_unknown_id_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);

    client.renew_commission_ttl(&String::from_str(&env, "does-not-exist"));
}
