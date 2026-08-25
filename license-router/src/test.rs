#![cfg(test)]

use super::*;
use quality_oracle::{QualityOracle, QualityOracleClient as RealOracleClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, BytesN, Env, FromVal, IntoVal, String,
};

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
    oracle.attest_quality(
        &curator,
        dataset_id,
        &score,
        &BytesN::from_array(env, &[7u8; 32]),
    );
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

#[test]
fn test_issue_research_license_free() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let registry = Address::generate(&env);
    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(&env, &router_id);
    router.initialize(&admin, &registry);

    let licensee = Address::generate(&env);
    let id = router.issue_license(
        &licensee,
        &String::from_str(&env, "ds_x"),
        &LicenseType::Research,
        &String::from_str(&env, "GLOBAL"),
        &1000,
        &0,
    );

    let license = router.get_license(&id);
    assert_eq!(license.fee_paid_stroops, 0);
    assert_eq!(license.license_type, LicenseType::Research);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_issue_commercial_license_insufficient_fee() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let registry = Address::generate(&env);
    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(&env, &router_id);
    router.initialize(&admin, &registry);

    let licensee = Address::generate(&env);
    router.issue_license(
        &licensee,
        &String::from_str(&env, "ds_x"),
        &LicenseType::Commercial,
        &String::from_str(&env, "GLOBAL"),
        &1000,
        &99_999_999,
    );
}

#[test]
fn test_license_expiry() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let registry = Address::generate(&env);
    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(&env, &router_id);
    router.initialize(&admin, &registry);

    let licensee = Address::generate(&env);

    env.ledger().with_mut(|l| l.sequence_number = 100);

    let id = router.issue_license(
        &licensee,
        &String::from_str(&env, "ds_x"),
        &LicenseType::Commercial,
        &String::from_str(&env, "GLOBAL"),
        &1000,
        &100_000_000,
    );

    assert!(router.is_license_valid(&id));

    env.ledger().with_mut(|l| l.sequence_number = 1101);

    assert!(!router.is_license_valid(&id));
}

#[test]
fn test_revoke_license() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let registry = Address::generate(&env);
    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(&env, &router_id);
    router.initialize(&admin, &registry);

    let licensee = Address::generate(&env);
    let id = router.issue_license(
        &licensee,
        &String::from_str(&env, "ds_x"),
        &LicenseType::Commercial,
        &String::from_str(&env, "GLOBAL"),
        &1000,
        &100_000_000,
    );

    assert!(router.is_license_valid(&id));

    // Mock admin auth only if needed, but we used mock_all_auths.
    router.revoke_license(&id);

    assert!(!router.is_license_valid(&id));

    let license = router.get_license(&id);
    assert_eq!(license.state, LicenseState::Revoked);
}


#[test]
fn test_upgrade() {
    let env = Env::default();
    env.mock_all_auths();
    
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, LicenseRouter);
    let client = LicenseRouterClient::new(&env, &contract_id);
    
    let dataset_registry = Address::generate(&env);
    
    client.initialize(&admin, &dataset_registry);
    
    let dummy_wasm: &[u8] = include_bytes!("../../test_data/dummy.wasm");
    let wasm_hash = env.deployer().upload_contract_wasm(dummy_wasm);
    
    client.upgrade(&wasm_hash);
}

#[test]
#[should_panic(expected = "not initialized")]
fn test_upgrade_unauthorized_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    
    let contract_id = env.register_contract(None, LicenseRouter);
    let client = LicenseRouterClient::new(&env, &contract_id);
    
    let dummy_wasm: &[u8] = include_bytes!("../../test_data/dummy.wasm");
    let wasm_hash = env.deployer().upload_contract_wasm(dummy_wasm);
    
    client.upgrade(&wasm_hash);
}
// ---------------------------------------------------------------------------
// Fee minimums, per licence type
//
// The table in the contract is the whole commercial policy of this layer, and
// only two of its four rows were exercised. Each type is checked at its exact
// minimum and one stroop below it, because an off-by-one here either gives
// away a commercial licence at NonProfit rates or rejects a correctly paid one.
// ---------------------------------------------------------------------------

/// A router with no oracle configured, so these tests measure fee handling
/// alone and not the quality multiplier applied on top of it.
fn plain_router(env: &Env) -> LicenseRouterClient<'_> {
    let admin = Address::generate(env);
    let registry = Address::generate(env);
    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(env, &router_id);
    router.initialize(&admin, &registry);
    router
}

fn issue_at(
    env: &Env,
    router: &LicenseRouterClient,
    license_type: LicenseType,
    fee: i128,
) -> String {
    router.issue_license(
        &Address::generate(env),
        &String::from_str(env, "ds_fee"),
        &license_type,
        &String::from_str(env, "GLOBAL"),
        &1000,
        &fee,
    )
}

#[test]
fn test_nonprofit_license_at_exact_minimum_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    let id = issue_at(&env, &router, LicenseType::NonProfit, 1_000_000); // 0.1 USDC
    let license = router.get_license(&id);

    assert_eq!(license.license_type, LicenseType::NonProfit);
    assert_eq!(license.fee_paid_stroops, 1_000_000);
    assert_eq!(license.state, LicenseState::Active);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_nonprofit_license_one_stroop_under_minimum_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    issue_at(&env, &router, LicenseType::NonProfit, 999_999);
}

#[test]
fn test_government_license_at_exact_minimum_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    let id = issue_at(&env, &router, LicenseType::Government, 10_000_000); // 1 USDC
    let license = router.get_license(&id);

    assert_eq!(license.license_type, LicenseType::Government);
    assert_eq!(license.fee_paid_stroops, 10_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_government_license_one_stroop_under_minimum_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    issue_at(&env, &router, LicenseType::Government, 9_999_999);
}

#[test]
fn test_commercial_license_at_exact_minimum_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    let id = issue_at(&env, &router, LicenseType::Commercial, 100_000_000); // 10 USDC
    assert_eq!(router.get_license(&id).fee_paid_stroops, 100_000_000);
}

#[test]
fn test_research_license_accepts_an_overpayment() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    // Research has a zero minimum, which is a floor and not a fixed price —
    // a licensee choosing to pay must not be turned away.
    let id = issue_at(&env, &router, LicenseType::Research, 5_000_000);
    let license = router.get_license(&id);

    assert_eq!(license.license_type, LicenseType::Research);
    assert_eq!(license.fee_paid_stroops, 5_000_000);
}

// ---------------------------------------------------------------------------
// Expiry boundary
// ---------------------------------------------------------------------------

#[test]
fn test_license_is_valid_on_its_expiry_ledger_and_invalid_the_next() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    env.ledger().with_mut(|l| l.sequence_number = 500);
    let id = issue_at(&env, &router, LicenseType::Commercial, 100_000_000);
    let expiry = router.get_license(&id).expiry_ledger;
    assert_eq!(expiry, 1500); // issued at 500, duration 1000

    // The comparison is inclusive, so the expiry ledger itself is the last
    // ledger the licence is good for.
    env.ledger().with_mut(|l| l.sequence_number = expiry - 1);
    assert!(router.is_license_valid(&id));

    env.ledger().with_mut(|l| l.sequence_number = expiry);
    assert!(router.is_license_valid(&id));

    env.ledger().with_mut(|l| l.sequence_number = expiry + 1);
    assert!(!router.is_license_valid(&id));
}

#[test]
fn test_zero_duration_license_expires_on_the_ledger_it_was_issued() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    env.ledger().with_mut(|l| l.sequence_number = 900);
    let id = router.issue_license(
        &Address::generate(&env),
        &String::from_str(&env, "ds_zero"),
        &LicenseType::Research,
        &String::from_str(&env, "GLOBAL"),
        &0,
        &0,
    );

    let license = router.get_license(&id);
    assert_eq!(license.issued_ledger, license.expiry_ledger);
    assert!(router.is_license_valid(&id));

    env.ledger().with_mut(|l| l.sequence_number = 901);
    assert!(!router.is_license_valid(&id));
}

#[test]
fn test_unknown_license_is_invalid_rather_than_panicking() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    // Callers gate on this, so it has to fail closed on an id that was never
    // issued rather than trapping the whole transaction.
    assert!(!router.is_license_valid(&String::from_str(&env, "lic_404")));
}

// ---------------------------------------------------------------------------
// Revocation is terminal
// ---------------------------------------------------------------------------

#[test]
fn test_revoked_license_is_not_revived_by_expiry_still_being_in_the_future() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    env.ledger().with_mut(|l| l.sequence_number = 100);
    let id = issue_at(&env, &router, LicenseType::Commercial, 100_000_000);
    router.revoke_license(&id);

    // Well inside the licence term, and still invalid: state is checked
    // before expiry, so an unexpired revoked licence stays revoked.
    env.ledger().with_mut(|l| l.sequence_number = 500);
    assert!(!router.is_license_valid(&id));
    assert_eq!(router.get_license(&id).state, LicenseState::Revoked);
}

#[test]
fn test_revoking_twice_leaves_the_license_revoked() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    let id = issue_at(&env, &router, LicenseType::Commercial, 100_000_000);
    router.revoke_license(&id);
    router.revoke_license(&id);

    assert_eq!(router.get_license(&id).state, LicenseState::Revoked);
    assert!(!router.is_license_valid(&id));
}

#[test]
fn test_reissuing_for_the_same_dataset_does_not_reactivate_a_revoked_license() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    let first = issue_at(&env, &router, LicenseType::Commercial, 100_000_000);
    router.revoke_license(&first);

    // A fresh licence for the same dataset is a new record with a new id; it
    // must not resurrect the revoked one.
    let second = issue_at(&env, &router, LicenseType::Commercial, 100_000_000);
    assert_ne!(first, second);

    assert!(!router.is_license_valid(&first));
    assert!(router.is_license_valid(&second));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_revoking_an_unknown_license_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    router.revoke_license(&String::from_str(&env, "lic_404"));
}

// ---------------------------------------------------------------------------
// Events
//
// Indexers and the off-chain royalty flow key off these topics, so the pair
// is asserted directly rather than assumed from the state change.
// ---------------------------------------------------------------------------

#[test]
fn test_issue_and_revoke_emit_their_documented_topics() {
    let env = Env::default();
    env.mock_all_auths();
    let router = plain_router(&env);

    let id = issue_at(&env, &router, LicenseType::Commercial, 100_000_000);
    let issued = env.events().all().last().unwrap();
    assert_eq!(
        issued.1,
        (symbol_short!("license"), symbol_short!("issued")).into_val(&env)
    );

    router.revoke_license(&id);
    let revoked = env.events().all().last().unwrap();
    assert_eq!(
        revoked.1,
        (symbol_short!("license"), symbol_short!("revoked")).into_val(&env)
    );
    assert_eq!(String::from_val(&env, &revoked.2), id);
}
