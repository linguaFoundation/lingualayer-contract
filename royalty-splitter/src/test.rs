#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, token, Address, Env, String, Vec};

#[test]
fn test_distribute_correct_amounts() {
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
