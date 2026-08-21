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

fn setup_disputable_commission(
    env: &Env,
) -> (DataCommissionClient<'static>, Address, Address, Address, String) {
    let commissioner = Address::generate(env);
    let bounty_token = env.register_stellar_asset_contract(commissioner.clone());
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(env, &contract_id);

    env.mock_all_auths();
    token::StellarAssetClient::new(env, &bounty_token).mint(&commissioner, &1000);

    let admin = Address::generate(env);
    client.initialize(&admin);

    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 2;
    let valid_hash = BytesN::from_array(env, &hash_bytes);

    let id = client.post_commission(
        &commissioner,
        &String::from_str(env, "en"),
        &valid_hash,
        &bounty_token,
        &1000,
        &100,
        &3600,
        &9999999,
    );

    (client, admin, commissioner, bounty_token, id)
}

#[test]
fn test_raise_and_resolve_dispute_in_fulfillers_favor() {
    let env = Env::default();
    let (client, admin, commissioner, token, id) = setup_disputable_commission(&env);

    let arbiter = Address::generate(&env);
    client.set_arbiter(&arbiter);

    client.raise_dispute(&id, &commissioner);
    assert_eq!(client.get_commission(&id).state, CommissionState::Disputed);

    let fulfiller = Address::generate(&env);
    let token_client = token::Client::new(&env, &token);
    let fulfiller_balance_before = token_client.balance(&fulfiller);

    client.resolve_dispute(&id, &true, &fulfiller, &String::from_str(&env, "ds_1"));

    let comm = client.get_commission(&id);
    assert_eq!(comm.state, CommissionState::Fulfilled);
    assert_eq!(comm.fulfiller, Some(fulfiller.clone()));
    assert_eq!(token_client.balance(&fulfiller) - fulfiller_balance_before, 1000);

    let _ = admin; // unused beyond initialize in this test
}

#[test]
fn test_raise_and_resolve_dispute_in_commissioners_favor_refunds() {
    let env = Env::default();
    let (client, _admin, commissioner, token, id) = setup_disputable_commission(&env);

    let arbiter = Address::generate(&env);
    client.set_arbiter(&arbiter);
    client.raise_dispute(&id, &commissioner);

    let token_client = token::Client::new(&env, &token);
    let commissioner_balance_before = token_client.balance(&commissioner);

    let fulfiller = Address::generate(&env);
    client.resolve_dispute(&id, &false, &fulfiller, &String::from_str(&env, "ds_1"));

    let comm = client.get_commission(&id);
    assert_eq!(comm.state, CommissionState::Cancelled);
    assert_eq!(token_client.balance(&commissioner) - commissioner_balance_before, 1000);
}

#[test]
#[should_panic(expected = "only the commissioner can raise a dispute")]
fn test_raise_dispute_by_non_commissioner_panics() {
    let env = Env::default();
    let (client, _admin, _commissioner, _token, id) = setup_disputable_commission(&env);

    let stranger = Address::generate(&env);
    client.raise_dispute(&id, &stranger);
}

#[test]
#[should_panic(expected = "commission not disputed")]
fn test_resolve_dispute_without_raising_panics() {
    let env = Env::default();
    let (client, _admin, _commissioner, _token, id) = setup_disputable_commission(&env);

    let arbiter = Address::generate(&env);
    client.set_arbiter(&arbiter);

    let fulfiller = Address::generate(&env);
    client.resolve_dispute(&id, &true, &fulfiller, &String::from_str(&env, "ds_1"));
}

#[test]
#[should_panic(expected = "no arbiter set")]
fn test_resolve_dispute_without_arbiter_set_panics() {
    let env = Env::default();
    let (client, _admin, commissioner, _token, id) = setup_disputable_commission(&env);

    client.raise_dispute(&id, &commissioner);

    let fulfiller = Address::generate(&env);
    client.resolve_dispute(&id, &true, &fulfiller, &String::from_str(&env, "ds_1"));
}
