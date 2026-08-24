#![cfg(test)]
//! Storage lifetime tests for licences.
//!
//! Licences are the longest-lived records in this workspace and the ones
//! least likely to be touched again after issuance — a three-year commercial
//! licence is written once and then read for years — so renewal without a
//! mutating call is the only thing keeping them alive.

use super::*;
use soroban_sdk::{
    testutils::{storage::Persistent as _, Address as _, Ledger as _},
    Address, Env, String,
};

fn setup(env: &Env) -> (LicenseRouterClient<'static>, Address) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let registry = Address::generate(env);
    let router_id = env.register_contract(None, LicenseRouter);
    let router = LicenseRouterClient::new(env, &router_id);
    router.initialize(&admin, &registry);
    (router, admin)
}

fn issue(env: &Env, router: &LicenseRouterClient, duration: u32) -> String {
    router.issue_license(
        &Address::generate(env),
        &String::from_str(env, "ds_ttl"),
        &LicenseType::Commercial,
        &String::from_str(env, "GLOBAL"),
        &duration,
        &100_000_000,
    )
}

/// `extend_ttl` asks for PERSISTENT_TTL but the host caps every entry at the
/// ledger's max_entry_ttl, so a full window is the smaller of the two.
fn full_window(env: &Env) -> u32 {
    core::cmp::min(PERSISTENT_TTL, env.ledger().get().max_entry_ttl - 1)
}

fn ttl_of(env: &Env, contract: &Address, key: &String) -> u32 {
    env.as_contract(contract, || env.storage().persistent().get_ttl(key))
}

fn advance(env: &Env, ledgers: u32) {
    env.ledger().with_mut(|l| l.sequence_number += ledgers);
}

#[test]
fn test_issue_license_extends_ttl_on_creation() {
    let env = Env::default();
    let (router, _admin) = setup(&env);
    let id = issue(&env, &router, 1000);

    assert_eq!(ttl_of(&env, &router.address, &id), full_window(&env));
}

#[test]
fn test_revoke_license_renews_ttl() {
    let env = Env::default();
    let (router, _admin) = setup(&env);
    let id = issue(&env, &router, 1000);

    advance(&env, 1_000_000);
    assert!(ttl_of(&env, &router.address, &id) < full_window(&env));

    router.revoke_license(&id);

    // The revocation record is the evidence the revocation happened; it must
    // not be the one entry left to lapse.
    assert_eq!(ttl_of(&env, &router.address, &id), full_window(&env));
    assert_eq!(router.get_license(&id).state, LicenseState::Revoked);
}

#[test]
fn test_renew_license_ttl_is_permissionless() {
    let env = Env::default();
    let (router, _admin) = setup(&env);
    let id = issue(&env, &router, 50_000_000);

    advance(&env, 2_000_000);
    assert!(ttl_of(&env, &router.address, &id) < full_window(&env));

    // Neither the licensee nor the admin: with every mock auth cleared, a
    // third party still gets through, because nothing on this path calls
    // require_auth.
    env.set_auths(&[]);
    router.renew_license_ttl(&id);

    assert_eq!(ttl_of(&env, &router.address, &id), full_window(&env));
}

#[test]
#[should_panic(expected = "license not found")]
fn test_renew_license_ttl_unknown_id_panics() {
    let env = Env::default();
    let (router, _admin) = setup(&env);
    router.renew_license_ttl(&String::from_str(&env, "lic_404"));
}

#[test]
fn test_multi_year_license_survives_renewal_across_long_ledger_advance() {
    let env = Env::default();
    let (router, _admin) = setup(&env);

    // A licence lasting far longer than one TTL window — precisely the case
    // that silently evaporates today.
    let id = issue(&env, &router, 60_000_000);
    let window = full_window(&env);

    for _ in 0..9 {
        advance(&env, window - 1);
        router.renew_license_ttl(&id);
    }

    // Still readable, still valid, and its terms are unchanged.
    let license = router.get_license(&id);
    assert_eq!(license.fee_paid_stroops, 100_000_000);
    assert_eq!(license.license_type, LicenseType::Commercial);
    assert!(router.is_license_valid(&id));
}

#[test]
fn test_renewal_does_not_resurrect_an_expired_license() {
    let env = Env::default();
    let (router, _admin) = setup(&env);
    let id = issue(&env, &router, 1000);

    advance(&env, 2000);
    assert!(!router.is_license_valid(&id));

    // Storage lifetime and licence validity are separate concerns: keeping
    // the record readable must not make a lapsed licence enforceable again.
    router.renew_license_ttl(&id);

    assert_eq!(ttl_of(&env, &router.address, &id), full_window(&env));
    assert!(!router.is_license_valid(&id));
}
