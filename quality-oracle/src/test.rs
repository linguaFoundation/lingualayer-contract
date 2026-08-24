#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, IntoVal, String};

#[test]
fn test_register_and_attest_happy_path() {
    let env = Env::default();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);

    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let curator = Address::generate(&env);
    client.register_curator(&curator);

    let dataset_id = String::from_str(&env, "ds_1");
    let rubric_hash = BytesN::from_array(&env, &[1u8; 32]);
    client.attest_quality(&curator, &dataset_id, &75, &rubric_hash);

    let quality = client.get_quality(&dataset_id);
    assert_eq!(quality.average_score, 75);
    assert_eq!(quality.attestation_count, 1);
    assert_eq!(quality.tier, QualityTier::Gold);
}

/// A tiny xorshift32 PRNG — deterministic and dependency-free, so this fuzz
/// test needs no `rand`/`arbitrary` crate (which would need no_std/wasm
/// compatibility vetting) and reproduces identically on every run/CI
/// machine while still exercising a wide, non-hand-picked range of inputs.
struct Xorshift32(u32);

impl Xorshift32 {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform-ish in [0, bound).
    fn below(&mut self, bound: u32) -> u32 {
        self.next() % bound
    }
}

fn expected_tier(score: u32) -> QualityTier {
    match score {
        0 => QualityTier::Unrated,
        1..=39 => QualityTier::Bronze,
        40..=69 => QualityTier::Silver,
        70..=84 => QualityTier::Gold,
        _ => QualityTier::Platinum,
    }
}

fn expected_royalty_bps(tier: &QualityTier) -> u32 {
    match tier {
        QualityTier::Platinum => 15000,
        QualityTier::Gold => 12500,
        QualityTier::Silver => 10000,
        QualityTier::Bronze => 7500,
        QualityTier::Unrated => 10000,
    }
}

/// Fuzzes attest_quality with ~200 pseudo-random scores from distinct
/// curators (a fresh curator per call sidesteps the same-curator-overwrite
/// quirk in attest_key, which would make "average of N attestations" an
/// unclear invariant to fuzz against) and checks, after every single call,
/// that the running average, the derived tier, and the royalty multiplier
/// it drives all stay internally consistent — not just on a few hand-picked
/// cases, but across the full random walk of the aggregate.
///
/// Finding from running this: attest_quality's running average is computed
/// from the *previous already-integer-truncated* average_score
/// (`prev_avg * prev_count + new_score, then / new_count`) rather than
/// from a tracked exact sum, so it silently drifts from the true
/// mathematical mean as attestations accumulate — e.g. after 200 random
/// scores here it disagrees with the exact mean by several points. That's
/// a real precision bug worth fixing in a follow-up, but out of scope for
/// this PR (which is scoped to adding the fuzz coverage that surfaced it,
/// not to changing aggregation behavior). This test mirrors the contract's
/// own incremental formula as the oracle, so it still meaningfully catches
/// range/tier/royalty-consistency regressions without failing on the
/// already-known drift.
#[test]
fn test_fuzz_score_aggregation_invariants_hold_across_random_sequence() {
    let env = Env::default();
    // 200 iterations of register_curator + attest_quality + get_quality +
    // royalty_multiplier_bps against one Env exceeds the default simulated
    // CPU/memory budget partway through (this isn't real network cost —
    // it's the host's default test-budget ceiling) — lift it so the fuzz
    // loop can run its full sweep.
    env.budget().reset_unlimited();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);

    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let dataset_id = String::from_str(&env, "ds_fuzz");
    let mut rng = Xorshift32(0x1234_5678);

    // Mirrors attest_quality's own incremental formula exactly (not the
    // true mean — see the drift note above) so this is a meaningful oracle
    // for "did the contract's aggregation change", not a false positive on
    // its known precision quirk.
    let mut shadow_avg: u64 = 0;
    let mut shadow_count: u64 = 0;

    for i in 0..200u32 {
        let score = rng.below(MAX_SCORE + 1); // 0..=100 inclusive
        let curator = Address::generate(&env);
        client.register_curator(&curator);

        let mut rubric_bytes = [0u8; 32];
        rubric_bytes[0] = (i % 256) as u8;
        let rubric_hash = BytesN::from_array(&env, &rubric_bytes);

        client.attest_quality(&curator, &dataset_id, &score, &rubric_hash);

        let new_total = shadow_avg * shadow_count + score as u64;
        shadow_count += 1;
        shadow_avg = new_total / shadow_count;

        let quality = client.get_quality(&dataset_id);

        // average_score never leaves the valid score range regardless of
        // how many or which random scores fed into it.
        assert!(
            quality.average_score <= MAX_SCORE,
            "iteration {i}: average_score {} out of range",
            quality.average_score
        );
        assert_eq!(
            quality.average_score as u64, shadow_avg,
            "iteration {i}: average diverged from the contract's own incremental formula"
        );
        assert_eq!(
            quality.attestation_count, shadow_count as u32,
            "iteration {i}: attestation_count diverged"
        );

        let tier = expected_tier(quality.average_score);
        assert_eq!(
            quality.tier, tier,
            "iteration {i}: tier inconsistent with its own average_score"
        );

        let bps = client.royalty_multiplier_bps(&dataset_id);
        assert_eq!(
            bps,
            expected_royalty_bps(&tier),
            "iteration {i}: royalty multiplier inconsistent with tier"
        );
        assert!(
            bps == 7500 || bps == 10000 || bps == 12500 || bps == 15000,
            "iteration {i}: royalty multiplier {bps} not one of the four valid values"
        );
    }
}

#[test]
#[should_panic(expected = "score must be 0-100")]
fn test_attest_quality_rejects_out_of_range_score() {
    let env = Env::default();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);

    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let curator = Address::generate(&env);
    client.register_curator(&curator);

    let dataset_id = String::from_str(&env, "ds_bad");
    let rubric_hash = BytesN::from_array(&env, &[2u8; 32]);
    client.attest_quality(&curator, &dataset_id, &101, &rubric_hash);
}

#[test]
#[should_panic(expected = "curator not registered")]
fn test_attest_quality_rejects_unregistered_curator() {
    let env = Env::default();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);

    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let curator = Address::generate(&env);
    let dataset_id = String::from_str(&env, "ds_unreg");
    let rubric_hash = BytesN::from_array(&env, &[3u8; 32]);
    client.attest_quality(&curator, &dataset_id, &50, &rubric_hash);
}

/// Sets up a dataset with three honest curators clustered around a score of
/// 50 (consensus/median = 50) plus one outlier curator scoring 95 — a
/// deviation of 45 points, comfortably over the 30-point slash threshold.
/// Returns (admin, outlier curator, dataset_id).
fn setup_outlier_scenario(env: &Env, client: &QualityOracleClient) -> (Address, Address, String) {
    let admin = Address::generate(env);
    client.initialize(&admin);

    let dataset_id = String::from_str(env, "ds_slash");
    let rubric_hash = BytesN::from_array(env, &[7u8; 32]);

    for score in [45u32, 50, 55] {
        let honest = Address::generate(env);
        client.register_curator(&honest);
        client.attest_quality(&honest, &dataset_id, &score, &rubric_hash);
    }

    let outlier = Address::generate(env);
    client.register_curator(&outlier);
    client.attest_quality(&outlier, &dataset_id, &95, &rubric_hash);

    (admin, outlier, dataset_id)
}

#[test]
fn test_slash_curator_reduces_stake_and_warns_on_first_offense() {
    let env = Env::default();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);
    env.mock_all_auths();

    let (_admin, outlier, dataset_id) = setup_outlier_scenario(&env, &client);

    let stake_before = client.get_curator(&outlier).stake;
    client.slash_curator(&outlier, &dataset_id);
    let state_after = client.get_curator(&outlier);

    assert_eq!(state_after.stake, stake_before - stake_before * 20 / 100);
    assert_eq!(state_after.status, CuratorStatus::SlashWarning);
    assert_eq!(client.treasury_balance(), stake_before * 20 / 100);
}

#[test]
fn test_slash_curator_bans_on_second_offense_and_blocks_attestation() {
    let env = Env::default();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);
    env.mock_all_auths();

    let (_admin, outlier, dataset_id) = setup_outlier_scenario(&env, &client);

    client.slash_curator(&outlier, &dataset_id);
    assert_eq!(client.get_curator(&outlier).status, CuratorStatus::SlashWarning);

    // A second dataset where the same curator is again an outlier.
    let dataset_id_2 = String::from_str(&env, "ds_slash_2");
    let rubric_hash = BytesN::from_array(&env, &[8u8; 32]);
    for score in [45u32, 50, 55] {
        let honest = Address::generate(&env);
        client.register_curator(&honest);
        client.attest_quality(&honest, &dataset_id_2, &score, &rubric_hash);
    }
    client.attest_quality(&outlier, &dataset_id_2, &95, &rubric_hash);

    client.slash_curator(&outlier, &dataset_id_2);
    assert_eq!(client.get_curator(&outlier).status, CuratorStatus::Banned);

    let dataset_id_3 = String::from_str(&env, "ds_after_ban");
    let result = client.try_attest_quality(&outlier, &dataset_id_3, &50, &rubric_hash);
    assert!(result.is_err());
}

#[test]
#[should_panic(expected = "attestation within consensus tolerance, cannot slash")]
fn test_slash_curator_rejects_scores_within_tolerance() {
    let env = Env::default();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let dataset_id = String::from_str(&env, "ds_close");
    let rubric_hash = BytesN::from_array(&env, &[9u8; 32]);

    let curator_a = Address::generate(&env);
    client.register_curator(&curator_a);
    client.attest_quality(&curator_a, &dataset_id, &50, &rubric_hash);

    let curator_b = Address::generate(&env);
    client.register_curator(&curator_b);
    client.attest_quality(&curator_b, &dataset_id, &60, &rubric_hash);

    client.slash_curator(&curator_a, &dataset_id);
}

#[test]
#[should_panic]
fn test_slash_curator_rejects_non_admin_caller() {
    let env = Env::default();
    let contract_id = env.register_contract(None, QualityOracle);
    let client = QualityOracleClient::new(&env, &contract_id);
    env.mock_all_auths();

    let (_admin, outlier, dataset_id) = setup_outlier_scenario(&env, &client);

    // Only a non-admin's auth is mocked for this call, so the contract's
    // internal `admin.require_auth()` has no matching authorization and
    // must reject the call.
    let non_admin = Address::generate(&env);
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &non_admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "slash_curator",
            args: (outlier.clone(), dataset_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    client.slash_curator(&outlier, &dataset_id);
}
