#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, testutils::Ledger, Address, Env, String};

/// Helper: create the contract client and initialize with a mock token for testing.
/// Returns (env, client, token_address, admin).
fn setup_env() -> (
    Env,
    MicroInsurancePoolClient<'static>,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    // Register the insurance pool contract
    let contract_id = env.register(MicroInsurancePool, ());
    let client = MicroInsurancePoolClient::new(&env, &contract_id);

    // Register a mock SAC token contract for testing
    let admin = Address::generate(&env);
    let token_address = env.register_stellar_asset_contract_v2(admin.clone()).address();

    // Mint initial tokens to use in tests
    let sac_admin = token::StellarAssetClient::new(&env, &token_address);
    // We'll mint to members individually in each test

    // Initialize the contract with:
    // - admin: the test admin
    // - token: the mock SAC token
    // - voting_period: 3600 seconds (1 hour)
    let voting_period: u64 = 3600;
    client.initialize(&admin, &token_address, &voting_period);

    (env, client, token_address, admin)
}

/// Helper: mint tokens to a member and have them join the pool
fn join_member(
    env: &Env,
    client: &MicroInsurancePoolClient,
    token_address: &Address,
    admin: &Address,
    member: &Address,
    premium: i128,
) {
    // Mint tokens to the member so they can pay the premium
    let sac_admin = token::StellarAssetClient::new(env, token_address);
    sac_admin.mint(member, &premium);

    // Member joins the pool
    client.join_pool(member, &premium);
}

// ============================================================================
// Test 1: Contract initialization
// ============================================================================
#[test]
fn test_initialization() {
    let (env, client, token_address, admin) = setup_env();

    // Pool should start with zero balance and zero members
    assert_eq!(client.get_pool_balance(), 0_i128);
    assert_eq!(client.get_member_count(), 0_u32);
}

// ============================================================================
// Test 2: Re-initialization is prevented
// ============================================================================
#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn test_cannot_reinitialize() {
    let (env, client, token_address, admin) = setup_env();

    // Trying to initialize again should fail with AlreadyInitialized (#10)
    client.initialize(&admin, &token_address, &3600);
}

// ============================================================================
// Test 3: Member joins the pool with real token transfer
// ============================================================================
#[test]
fn test_join_pool_with_token_transfer() {
    let (env, client, token_address, admin) = setup_env();
    let member = Address::generate(&env);

    // Mint 5000 tokens to the member
    let sac_admin = token::StellarAssetClient::new(&env, &token_address);
    sac_admin.mint(&member, &5000_i128);

    // Check initial token balance
    let token_client = token::Client::new(&env, &token_address);
    assert_eq!(token_client.balance(&member), 5000_i128);

    // Member joins with premium of 1000
    client.join_pool(&member, &1000_i128);

    // Pool balance should reflect the premium
    assert_eq!(client.get_pool_balance(), 1000_i128);

    // Member's token balance should decrease by premium
    assert_eq!(token_client.balance(&member), 4000_i128);

    // Contract should hold the tokens
    let contract_addr = client.address.clone();
    assert_eq!(token_client.balance(&contract_addr), 1000_i128);

    // Reputation should be initialized to 100
    assert_eq!(client.get_reputation(&member), 100_i128);

    // Member count should be 1
    assert_eq!(client.get_member_count(), 1_u32);
}

// ============================================================================
// Test 4: Full claim lifecycle — file, vote approve, execute with token payout
// ============================================================================
#[test]
fn test_file_and_approve_claim_with_payout() {
    let (env, client, token_address, admin) = setup_env();

    let member_a = Address::generate(&env);
    let member_b = Address::generate(&env);
    let member_c = Address::generate(&env);

    // Three members join (need 3 for quorum since claimant can't vote)
    join_member(&env, &client, &token_address, &admin, &member_a, 5000);
    join_member(&env, &client, &token_address, &admin, &member_b, 5000);
    join_member(&env, &client, &token_address, &admin, &member_c, 5000);

    assert_eq!(client.get_pool_balance(), 15_000_i128);
    assert_eq!(client.get_member_count(), 3_u32);

    // Member A files a claim for 3,000
    let reason = String::from_str(&env, "Medical emergency");
    let claim_id = client.file_claim(&member_a, &3000_i128, &reason);
    assert_eq!(claim_id, 0);

    // Members B and C vote to approve (member A cannot self-vote)
    client.vote_claim(&member_b, &claim_id, &true);
    client.vote_claim(&member_c, &claim_id, &true);

    // Advance ledger time past the voting deadline (1 hour + 1 second)
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });

    // Execute the claim — should be approved (2 approve, 0 reject)
    client.execute_claim(&claim_id);

    // Pool balance should decrease by the claim amount
    assert_eq!(client.get_pool_balance(), 12_000_i128);

    // Verify real token transfer: member A should have received 3,000 tokens
    let token_client = token::Client::new(&env, &token_address);
    assert_eq!(token_client.balance(&member_a), 3000_i128); // 0 remaining + 3000 payout

    // Claim status should be Executed
    let claim = client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::Executed);
    assert_eq!(claim.votes_approve, 2);
    assert_eq!(claim.votes_reject, 0);

    // Claimant reputation should increase by 5 (100 + 5 = 105)
    assert_eq!(client.get_reputation(&member_a), 105_i128);
}

// ============================================================================
// Test 5: Claim is rejected by majority vote
// ============================================================================
#[test]
fn test_reject_claim() {
    let (env, client, token_address, admin) = setup_env();

    let member_a = Address::generate(&env);
    let member_b = Address::generate(&env);
    let member_c = Address::generate(&env);

    join_member(&env, &client, &token_address, &admin, &member_a, 5000);
    join_member(&env, &client, &token_address, &admin, &member_b, 5000);
    join_member(&env, &client, &token_address, &admin, &member_c, 5000);

    // Member A files a claim
    let reason = String::from_str(&env, "Suspicious claim");
    let claim_id = client.file_claim(&member_a, &4000_i128, &reason);

    // B and C both reject
    client.vote_claim(&member_b, &claim_id, &false);
    client.vote_claim(&member_c, &claim_id, &false);

    // Advance past the deadline
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });

    // Execute — should be rejected (0 approve, 2 reject)
    client.execute_claim(&claim_id);

    // Pool balance should remain unchanged
    assert_eq!(client.get_pool_balance(), 15_000_i128);

    // Claim status should be Rejected
    let claim = client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::Rejected);

    // Claimant reputation should decrease by 10 (100 - 10 = 90)
    assert_eq!(client.get_reputation(&member_a), 90_i128);
}

// ============================================================================
// Test 6: Claimant cannot vote on their own claim (self-vote prevention)
// ============================================================================
#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn test_self_vote_prevented() {
    let (env, client, token_address, admin) = setup_env();

    let member_a = Address::generate(&env);
    let member_b = Address::generate(&env);

    join_member(&env, &client, &token_address, &admin, &member_a, 5000);
    join_member(&env, &client, &token_address, &admin, &member_b, 5000);

    let reason = String::from_str(&env, "Test claim");
    let claim_id = client.file_claim(&member_a, &1000_i128, &reason);

    // Member A tries to vote on their own claim — should panic with SelfVoteNotAllowed (#12)
    client.vote_claim(&member_a, &claim_id, &true);
}

// ============================================================================
// Test 7: Double voting is prevented
// ============================================================================
#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_double_vote_prevented() {
    let (env, client, token_address, admin) = setup_env();

    let member_a = Address::generate(&env);
    let member_b = Address::generate(&env);

    join_member(&env, &client, &token_address, &admin, &member_a, 5000);
    join_member(&env, &client, &token_address, &admin, &member_b, 5000);

    let reason = String::from_str(&env, "Test claim");
    let claim_id = client.file_claim(&member_a, &1000_i128, &reason);

    // Member B votes once — should succeed
    client.vote_claim(&member_b, &claim_id, &true);

    // Member B tries to vote again — should panic with AlreadyVoted (#7)
    client.vote_claim(&member_b, &claim_id, &false);
}

// ============================================================================
// Test 8: Non-member cannot file a claim
// ============================================================================
#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_non_member_cannot_file_claim() {
    let (env, client, token_address, admin) = setup_env();

    let outsider = Address::generate(&env);
    let reason = String::from_str(&env, "Fraudulent claim");
    client.file_claim(&outsider, &1000_i128, &reason);
}

// ============================================================================
// Test 9: Cannot execute claim before voting deadline
// ============================================================================
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_cannot_execute_before_deadline() {
    let (env, client, token_address, admin) = setup_env();

    let member_a = Address::generate(&env);
    let member_b = Address::generate(&env);

    join_member(&env, &client, &token_address, &admin, &member_a, 5000);
    join_member(&env, &client, &token_address, &admin, &member_b, 5000);

    let reason = String::from_str(&env, "Test deadline");
    let claim_id = client.file_claim(&member_a, &1000_i128, &reason);
    client.vote_claim(&member_b, &claim_id, &true);

    // Try to execute immediately (before deadline) — should panic with VotingPeriodActive (#14)
    client.execute_claim(&claim_id);
}

// ============================================================================
// Test 10: Cannot vote after voting deadline
// ============================================================================
#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_cannot_vote_after_deadline() {
    let (env, client, token_address, admin) = setup_env();

    let member_a = Address::generate(&env);
    let member_b = Address::generate(&env);

    join_member(&env, &client, &token_address, &admin, &member_a, 5000);
    join_member(&env, &client, &token_address, &admin, &member_b, 5000);

    let reason = String::from_str(&env, "Test deadline");
    let claim_id = client.file_claim(&member_a, &1000_i128, &reason);

    // Advance past the deadline
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });

    // Try to vote after deadline — should panic with VotingPeriodEnded (#13)
    client.vote_claim(&member_b, &claim_id, &true);
}
