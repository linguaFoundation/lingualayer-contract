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
fn test_admin_handoff_propose_then_accept() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);

    client.initialize(&admin);
    client.propose_admin(&new_admin);
    client.accept_admin();

    let another = Address::generate(&env);
    client.propose_admin(&another);
}

#[test]
#[should_panic(expected = "no admin proposal pending")]
fn test_accept_admin_without_proposal_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);

    client.initialize(&admin);
    client.accept_admin();
}
fn setup_commission(env: &Env) -> (DataCommissionClient<'static>, Address, Address, Address, String) {
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
    hash_bytes[0] = 1;
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
fn milestone(env: &Env, amount: i128) -> Milestone {
    let mut hash_bytes = [0u8; 32];
    hash_bytes[0] = 7;
    Milestone {
        description_hash: BytesN::from_array(env, &hash_bytes),
        amount,
        released: false,
    }
}

#[test]
fn test_set_milestones_and_release_pays_out_per_tranche() {
    let env = Env::default();
    let (client, _admin, _commissioner, _token, id) = setup_commission(&env);

    let mut milestones = soroban_sdk::Vec::new(&env);
    milestones.push_back(milestone(&env, 600));
    milestones.push_back(milestone(&env, 400));
    client.set_milestones(&id, &milestones);

    let fulfiller = Address::generate(&env);
    client.fulfil_commission(&id, &fulfiller, &String::from_str(&env, "ds_1"));

    // Milestone commissions stay Open (not Fulfilled) until every tranche
    // is released — fulfil_commission only assigns the fulfiller here.
    let mid = client.get_commission(&id);
    assert_eq!(mid.state, CommissionState::Open);

    client.release_milestone(&id, &0);
    let after_first = client.get_commission(&id);
    assert_eq!(after_first.state, CommissionState::Open);
    assert!(after_first.milestones.get(0).unwrap().released);
    assert!(!after_first.milestones.get(1).unwrap().released);

    client.release_milestone(&id, &1);
    let after_second = client.get_commission(&id);
    assert_eq!(after_second.state, CommissionState::Fulfilled);
}

#[test]
#[should_panic(expected = "milestone amounts must sum to the bounty amount")]
fn test_set_milestones_rejects_mismatched_total() {
    let env = Env::default();
    let (client, _admin, _commissioner, _token, id) = setup_commission(&env);

    let mut milestones = soroban_sdk::Vec::new(&env);
    milestones.push_back(milestone(&env, 600));
    milestones.push_back(milestone(&env, 300)); // sums to 900, not 1000
    client.set_milestones(&id, &milestones);
}

#[test]
#[should_panic(expected = "milestone already released")]
fn test_release_milestone_twice_panics() {
    let env = Env::default();
    let (client, _admin, _commissioner, _token, id) = setup_commission(&env);

    // Two milestones (not one) so the first release doesn't already flip
    // the commission to Fulfilled — that would make the second call hit
    // the "commission not open" guard instead of the one this test targets.
    let mut milestones = soroban_sdk::Vec::new(&env);
    milestones.push_back(milestone(&env, 600));
    milestones.push_back(milestone(&env, 400));
    client.set_milestones(&id, &milestones);

    let fulfiller = Address::generate(&env);
    client.fulfil_commission(&id, &fulfiller, &String::from_str(&env, "ds_1"));

    client.release_milestone(&id, &0);
    client.release_milestone(&id, &0);
}

#[test]
fn test_cancel_after_partial_milestone_release_refunds_only_remainder() {
    let env = Env::default();
    let (client, _admin, commissioner, token, id) = setup_commission(&env);

    let mut milestones = soroban_sdk::Vec::new(&env);
    milestones.push_back(milestone(&env, 600));
    milestones.push_back(milestone(&env, 400));
    client.set_milestones(&id, &milestones);

    let fulfiller = Address::generate(&env);
    client.fulfil_commission(&id, &fulfiller, &String::from_str(&env, "ds_1"));
    client.release_milestone(&id, &0);

    let token_client = token::Client::new(&env, &token);
    let commissioner_balance_before = token_client.balance(&commissioner);

    client.cancel_commission(&id);

    let commissioner_balance_after = token_client.balance(&commissioner);
    assert_eq!(commissioner_balance_after - commissioner_balance_before, 400);
}
