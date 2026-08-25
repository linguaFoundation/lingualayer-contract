#![cfg(test)]
//! Emergency pause: freeze every state-mutating entry point, keep reads live.
//!
//! Kept in its own module so the behavioural suite in `test` is untouched.

use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup() -> (Env, DataCommissionClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, client, admin)
}

#[test]
fn test_a_fresh_contract_is_not_paused() {
    let (_env, client, _admin) = setup();
    assert!(!client.is_paused());
}

#[test]
fn test_pause_then_unpause_round_trips() {
    let (_env, client, _admin) = setup();

    client.pause();
    assert!(client.is_paused());

    client.unpause();
    assert!(!client.is_paused());
}

#[test]
fn test_a_write_is_rejected_while_paused() {
    let (env, client, admin) = setup();
    let _ = (&env, &admin);

    client.pause();
    assert_eq!(
        client.try_set_arbiter(&Address::generate(&env)),
        Err(Ok(Error::ContractPaused))
    );
}

#[test]
fn test_the_same_write_succeeds_once_unpaused() {
    let (env, client, admin) = setup();
    let _ = (&env, &admin);

    client.pause();
    assert_eq!(
        client.try_set_arbiter(&Address::generate(&env)),
        Err(Ok(Error::ContractPaused))
    );

    // The whole point of the mechanism: the freeze is reversible and leaves
    // the contract working exactly as it did before.
    client.unpause();
    client.set_arbiter(&Address::generate(&env));
    assert!(!client.is_paused());
}

#[test]
fn test_reads_still_answer_while_paused() {
    let (_env, client, _admin) = setup();
    client.pause();

    // Integrators and the front end have to keep answering questions about
    // existing state during an incident; a read cannot make things worse.
    assert!(client.is_paused());
    assert_eq!(client.version(), 2);
}

#[test]
fn test_a_non_admin_cannot_pause() {
    let env = Env::default();
    let contract_id = env.register_contract(None, DataCommission);
    let client = DataCommissionClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    // Drop the blanket auth mock: pause() now has to satisfy the admin's own
    // require_auth, which an unauthorized caller cannot produce.
    env.set_auths(&[]);
    assert!(client.try_pause().is_err());
    assert!(!client.is_paused());
}

#[test]
fn test_pausing_twice_is_rejected() {
    let (_env, client, _admin) = setup();
    client.pause();
    assert_eq!(client.try_pause(), Err(Ok(Error::AlreadyPaused)));
    // Still paused - the rejected call changed nothing.
    assert!(client.is_paused());
}

#[test]
fn test_unpausing_a_live_contract_is_rejected() {
    let (_env, client, _admin) = setup();
    assert_eq!(client.try_unpause(), Err(Ok(Error::NotPaused)));
    assert!(!client.is_paused());
}
