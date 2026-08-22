#![cfg(test)]

use super::*;
use quality_oracle::{QualityOracle, QualityOracleClient as RealOracleClient};
use soroban_sdk::{testutils::Address as _, token, Address, BytesN, Env, String, Vec};

fn setup(env: &Env) -> (RoyaltySplitterClient<'_>, RealOracleClient<'_>, Address, Address) {
    let admin = Address::generate(env);

    let splitter_id = env.register_contract(None, RoyaltySplitter);
    let splitter = RoyaltySplitterClient::new(env, &splitter_id);
    splitter.initialize(&admin);

    let oracle_id = env.register_contract(None, QualityOracle);
    let oracle = RealOracleClient::new(env, &oracle_id);
    oracle.initialize(&admin);

    splitter.set_oracle(&oracle_id);

    (splitter, oracle, admin, splitter_id)
}

fn setup_split(
    env: &Env,
    splitter: &RoyaltySplitterClient,
    splitter_id: &Address,
    dataset_id: &String,
    contributor: &Address,
) -> Address {
    let treasury = Address::generate(env);
    let token_admin = Address::generate(env);
    // Same pattern as data-commission's tests: register a SAC with a
    // throwaway address as issuer, mint into whoever needs a spendable
    // balance in the scenario under test (here, the splitter contract
    // itself, since distribute() transfers *from* its own balance).
    let token_id = env.register_stellar_asset_contract(token_admin.clone());
    let token_admin_client = token::StellarAssetClient::new(env, &token_id);
    token_admin_client.mint(splitter_id, &1_000_000_000);

    let mut contributors = Vec::new(env);
    contributors.push_back((contributor.clone(), 10000u32));

    splitter.register_split(&SplitConfig {
        dataset_id: dataset_id.clone(),
        token: token_id.clone(),
        treasury,
        contributors,
    });

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
