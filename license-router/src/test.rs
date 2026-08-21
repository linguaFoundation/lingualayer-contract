#![cfg(test)]

use super::*;
use quality_oracle::{QualityOracle, QualityOracleClient as RealOracleClient};
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, String};

fn setup(env: &Env) -> (LicenseRouterClient<'_>, RealOracleClient<'_>, Address) {
    let admin = Address::generate(env);
    let registry = Address::generate(env);

    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(env, &router_id);
    router.initialize(&admin, &registry);

    let oracle_id = env.register_contract(None, QualityOracle);
    let oracle = RealOracleClient::new(env, &oracle_id);
    oracle.initialize(&admin);

    router.set_oracle(&oracle_id);

    (router, oracle, admin)
}

fn attest(env: &Env, oracle: &RealOracleClient, dataset_id: &String, score: u32) {
    let curator = Address::generate(env);
    oracle.register_curator(&curator);
    oracle.attest_quality(&curator, dataset_id, &score, &BytesN::from_array(env, &[7u8; 32]));
}

#[test]
fn test_platinum_dataset_gets_fifty_percent_royalty_premium() {
    let env = Env::default();
    env.mock_all_auths();
    let (router, oracle, _admin) = setup(&env);
    let dataset_id = String::from_str(&env, "ds_platinum");
    attest(&env, &oracle, &dataset_id, 95); // Platinum: 85-100

    let licensee = Address::generate(&env);
    let id = router.issue_license(
        &licensee,
        &dataset_id,
        &LicenseType::Commercial,
        &String::from_str(&env, "GLOBAL"),
        &1000,
        &100_000_000,
    );

    let license = router.get_license(&id);
    assert_eq!(license.fee_paid_stroops, 100_000_000);
    assert_eq!(license.effective_royalty_stroops, 150_000_000); // 1.5x
}

#[test]
fn test_unrated_dataset_gets_standard_royalties() {
    let env = Env::default();
    env.mock_all_auths();
    let (router, _oracle, _admin) = setup(&env);
    let dataset_id = String::from_str(&env, "ds_unrated"); // never attested

    let licensee = Address::generate(&env);
    let id = router.issue_license(
        &licensee,
        &dataset_id,
        &LicenseType::Commercial,
        &String::from_str(&env, "GLOBAL"),
        &1000,
        &100_000_000,
    );

    let license = router.get_license(&id);
    assert_eq!(license.effective_royalty_stroops, license.fee_paid_stroops); // 1x
}

#[test]
fn test_no_oracle_configured_defaults_to_standard_royalties() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let registry = Address::generate(&env);
    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(&env, &router_id);
    router.initialize(&admin, &registry); // no set_oracle call

    let licensee = Address::generate(&env);
    let id = router.issue_license(
        &licensee,
        &String::from_str(&env, "ds_x"),
        &LicenseType::Commercial,
        &String::from_str(&env, "GLOBAL"),
        &1000,
        &100_000_000,
    );

    let license = router.get_license(&id);
    assert_eq!(license.effective_royalty_stroops, license.fee_paid_stroops);
}
