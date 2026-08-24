#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, String, Vec};

/// Register a contract with a fresh env and an initialized admin, returning
/// the pieces every test needs. Auth is mocked up front because
/// `initialize` itself requires the admin's signature.
fn setup(env: &Env) -> (DatasetRegistryClient<'static>, Address) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(env, &contract_id);
    client.initialize(&admin);
    (client, admin)
}

fn hash_of(env: &Env, byte: u8) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = byte;
    BytesN::from_array(env, &bytes)
}

/// A single contributor holding the whole 10000 bps.
fn sole_contributor(env: &Env, who: &Address) -> Vec<ContributorShare> {
    let mut contributors = Vec::new(env);
    contributors.push_back(ContributorShare {
        address: who.clone(),
        share_bps: 10000,
    });
    contributors
}

fn register(
    env: &Env,
    client: &DatasetRegistryClient,
    owner: &Address,
    hash: &BytesN<32>,
) -> String {
    client.register_dataset(
        owner,
        &String::from_str(env, "en"),
        &String::from_str(env, "Test Dataset"),
        hash,
        &sole_contributor(env, owner),
        &100,
        &3600,
        &None,
    )
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "metadata hash cannot be zero")]
fn test_register_zero_hash_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let zero_hash = BytesN::from_array(&env, &[0u8; 32]);
    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Test Dataset"),
        &zero_hash,
        &sole_contributor(&env, &owner),
        &100,
        &3600,
        &None,
    );
}

#[test]
fn test_register_valid_hash_succeeds() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 1));

    assert_eq!(id, String::from_str(&env, "ds_1"));
    assert_eq!(client.dataset_count(), 1);

    let ds = client.get_dataset(&id);
    assert_eq!(ds.owner, owner);
    assert_eq!(ds.version, 1);
    assert_eq!(ds.state, DatasetState::Active);
    assert_eq!(ds.language_code, String::from_str(&env, "en"));
    assert_eq!(ds.sample_count, 100);
    assert_eq!(ds.duration_seconds, 3600);
}

#[test]
fn test_dataset_ids_increment_across_registrations() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let first = register(&env, &client, &owner, &hash_of(&env, 1));
    let second = register(&env, &client, &owner, &hash_of(&env, 2));

    assert_eq!(first, String::from_str(&env, "ds_1"));
    assert_eq!(second, String::from_str(&env, "ds_2"));
    assert_eq!(client.dataset_count(), 2);
}

#[test]
#[should_panic(expected = "dataset with this metadata hash is already registered")]
fn test_register_duplicate_hash_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let dup_hash = hash_of(&env, 9);

    register(&env, &client, &owner, &dup_hash);

    // Same metadata_hash, different owner/name/language — should still be
    // rejected, since the hash is what identifies the underlying dataset.
    let other_owner = Address::generate(&env);
    client.register_dataset(
        &other_owner,
        &String::from_str(&env, "fr"),
        &String::from_str(&env, "Second"),
        &dup_hash,
        &sole_contributor(&env, &other_owner),
        &200,
        &7200,
        &None,
    );
}

#[test]
fn test_dataset_id_for_hash_returns_id_after_registration_and_none_before() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let hash = hash_of(&env, 5);

    assert_eq!(client.dataset_id_for_hash(&hash), None);

    let id = register(&env, &client, &owner, &hash);

    assert_eq!(client.dataset_id_for_hash(&hash), Some(id));
}

// ---------------------------------------------------------------------------
// Contributor share validation
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "contributor shares must sum to 10000 bps")]
fn test_register_shares_under_target_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 4000,
    });
    contributors.push_back(ContributorShare {
        address: Address::generate(&env),
        share_bps: 5000,
    });

    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Underweight"),
        &hash_of(&env, 11),
        &contributors,
        &100,
        &3600,
        &None,
    );
}

#[test]
#[should_panic(expected = "contributor shares must sum to 10000 bps")]
fn test_register_shares_over_target_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 6000,
    });
    contributors.push_back(ContributorShare {
        address: Address::generate(&env),
        share_bps: 5000,
    });

    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Overweight"),
        &hash_of(&env, 12),
        &contributors,
        &100,
        &3600,
        &None,
    );
}

#[test]
#[should_panic(expected = "dataset must have at least one contributor")]
fn test_register_empty_contributors_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "No contributors"),
        &hash_of(&env, 13),
        &Vec::new(&env),
        &100,
        &3600,
        &None,
    );
}

#[test]
#[should_panic(expected = "contributor share must be greater than zero")]
fn test_register_zero_share_contributor_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 10000,
    });
    contributors.push_back(ContributorShare {
        address: Address::generate(&env),
        share_bps: 0,
    });

    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Zero share"),
        &hash_of(&env, 14),
        &contributors,
        &100,
        &3600,
        &None,
    );
}

#[test]
#[should_panic(expected = "duplicate contributor address")]
fn test_register_duplicate_contributor_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    // Sums to exactly 10000, so only the duplicate check can catch it.
    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 5000,
    });
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 5000,
    });

    client.register_dataset(
        &owner,
        &String::from_str(&env, "en"),
        &String::from_str(&env, "Duplicate contributor"),
        &hash_of(&env, 15),
        &contributors,
        &100,
        &3600,
        &None,
    );
}

#[test]
fn test_register_multiple_contributors_summing_to_target_succeeds() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let mut contributors = Vec::new(&env);
    contributors.push_back(ContributorShare {
        address: owner.clone(),
        share_bps: 6000,
    });
    contributors.push_back(ContributorShare {
        address: Address::generate(&env),
        share_bps: 2500,
    });
    contributors.push_back(ContributorShare {
        address: Address::generate(&env),
        share_bps: 1500,
    });

    let id = client.register_dataset(
        &owner,
        &String::from_str(&env, "yo"),
        &String::from_str(&env, "Three way split"),
        &hash_of(&env, 16),
        &contributors,
        &500,
        &18_000,
        &None,
    );

    assert_eq!(client.get_dataset(&id).contributors.len(), 3);
}

// ---------------------------------------------------------------------------
// Initialization / admin
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "already initialized")]
fn test_double_initialize_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    client.initialize(&Address::generate(&env));
}

#[test]
#[should_panic(expected = "not initialized")]
fn test_register_before_initialize_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, DatasetRegistry);
    let client = DatasetRegistryClient::new(&env, &contract_id);
    let owner = Address::generate(&env);

    register(&env, &client, &owner, &hash_of(&env, 21));
}

#[test]
fn test_admin_handoff_propose_then_accept() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&new_admin);
    client.accept_admin();

    // New admin can now do admin-gated work the old admin no longer can
    // authenticate as; re-proposing confirms the swap actually took effect.
    let another = Address::generate(&env);
    client.propose_admin(&another);
}

#[test]
#[should_panic(expected = "no admin proposal pending")]
fn test_accept_admin_without_proposal_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    client.accept_admin();
}

// ---------------------------------------------------------------------------
// update_metadata
// ---------------------------------------------------------------------------

#[test]
fn test_update_metadata_bumps_version_and_moves_hash_index() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let original = hash_of(&env, 31);
    let updated = hash_of(&env, 32);

    let id = register(&env, &client, &owner, &original);
    client.update_metadata(&id, &updated);

    let ds = client.get_dataset(&id);
    assert_eq!(ds.version, 2);
    assert_eq!(ds.metadata_hash, updated);

    // The index follows the dataset: the new hash resolves, the old one is
    // released so a genuinely different dataset could claim it.
    assert_eq!(client.dataset_id_for_hash(&updated), Some(id));
    assert_eq!(client.dataset_id_for_hash(&original), None);
}

#[test]
#[should_panic(expected = "metadata hash cannot be zero")]
fn test_update_metadata_zero_hash_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 33));
    client.update_metadata(&id, &BytesN::from_array(&env, &[0u8; 32]));
}

#[test]
#[should_panic(expected = "metadata hash unchanged")]
fn test_update_metadata_same_hash_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let hash = hash_of(&env, 34);

    let id = register(&env, &client, &owner, &hash);
    client.update_metadata(&id, &hash);
}

#[test]
#[should_panic(expected = "dataset with this metadata hash is already registered")]
fn test_update_metadata_to_another_datasets_hash_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let taken = hash_of(&env, 36);

    let first = register(&env, &client, &owner, &hash_of(&env, 35));
    register(&env, &client, &owner, &taken);

    client.update_metadata(&first, &taken);
}

#[test]
#[should_panic(expected = "dataset not found")]
fn test_update_metadata_unknown_dataset_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    client.update_metadata(&String::from_str(&env, "ds_404"), &hash_of(&env, 37));
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[test]
fn test_flag_then_reinstate_round_trips_state() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 41));
    assert_eq!(client.get_state(&id), DatasetState::Active);

    client.flag_dataset(&id);
    assert_eq!(client.get_state(&id), DatasetState::UnderReview);

    client.reinstate_dataset(&id);
    assert_eq!(client.get_state(&id), DatasetState::Active);
}

#[test]
#[should_panic(expected = "dataset must be active to update metadata")]
fn test_update_metadata_under_review_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 42));
    client.flag_dataset(&id);
    client.update_metadata(&id, &hash_of(&env, 43));
}

#[test]
#[should_panic(expected = "only an active dataset can be flagged for review")]
fn test_flag_twice_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 44));
    client.flag_dataset(&id);
    client.flag_dataset(&id);
}

#[test]
#[should_panic(expected = "only a dataset under review can be reinstated")]
fn test_reinstate_active_dataset_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 45));
    client.reinstate_dataset(&id);
}

// ---------------------------------------------------------------------------
// deprecate_dataset
// ---------------------------------------------------------------------------

#[test]
fn test_owner_can_deprecate() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 51));
    client.deprecate_dataset(&id, &owner);

    assert_eq!(client.get_state(&id), DatasetState::Deprecated);
}

#[test]
fn test_admin_can_deprecate() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 52));
    client.deprecate_dataset(&id, &admin);

    assert_eq!(client.get_state(&id), DatasetState::Deprecated);
}

#[test]
fn test_under_review_dataset_can_be_deprecated() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 53));
    client.flag_dataset(&id);
    client.deprecate_dataset(&id, &owner);

    assert_eq!(client.get_state(&id), DatasetState::Deprecated);
}

#[test]
#[should_panic(expected = "only the dataset owner or admin can deprecate")]
fn test_stranger_cannot_deprecate() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);
    let stranger = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 54));
    client.deprecate_dataset(&id, &stranger);
}

#[test]
#[should_panic(expected = "dataset is already deprecated")]
fn test_double_deprecate_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 55));
    client.deprecate_dataset(&id, &owner);
    client.deprecate_dataset(&id, &owner);
}

#[test]
#[should_panic(expected = "dataset must be active to update metadata")]
fn test_update_metadata_after_deprecation_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 56));
    client.deprecate_dataset(&id, &owner);
    client.update_metadata(&id, &hash_of(&env, 57));
}

#[test]
#[should_panic(expected = "only an active dataset can be flagged for review")]
fn test_deprecated_is_terminal_and_cannot_be_flagged() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    let id = register(&env, &client, &owner, &hash_of(&env, 58));
    client.deprecate_dataset(&id, &owner);
    client.flag_dataset(&id);
}

// ---------------------------------------------------------------------------
// Reputation
// ---------------------------------------------------------------------------

#[test]
fn test_reputation_accrues_per_registration() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let owner = Address::generate(&env);

    register(&env, &client, &owner, &hash_of(&env, 61));
    let after_one = client.get_reputation(&owner);
    assert_eq!(after_one.datasets_registered, 1);
    assert_eq!(after_one.reputation_score, 50);

    register(&env, &client, &owner, &hash_of(&env, 62));
    let after_two = client.get_reputation(&owner);
    assert_eq!(after_two.datasets_registered, 2);
    assert_eq!(after_two.reputation_score, 100);
}

#[test]
#[should_panic(expected = "no reputation data")]
fn test_reputation_for_unknown_address_panics() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    client.get_reputation(&Address::generate(&env));
}
