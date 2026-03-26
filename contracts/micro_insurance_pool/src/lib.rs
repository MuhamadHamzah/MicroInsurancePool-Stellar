#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env,
    String, Symbol,
};

// ============================================================================
// Data Types
// ============================================================================

/// Represents the current status of an insurance claim.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum ClaimStatus {
    Pending,
    Approved,
    Rejected,
    Executed,
}

/// Represents an insurance claim filed by a pool member.
/// Includes a voting deadline for time-bound governance.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Claim {
    pub claimant: Address,
    pub amount: i128,
    pub reason: String,
    pub votes_approve: u32,
    pub votes_reject: u32,
    pub status: ClaimStatus,
    /// Ledger timestamp after which voting closes and execution is allowed.
    pub deadline: u64,
}

/// Storage keys for all contract data.
/// Using an enum with #[contracttype] gives us type-safe, unique storage keys.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Whether the contract has been initialized
    Initialized,
    /// Admin address (set once during initialization)
    Admin,
    /// The SAC token contract address used for premiums and payouts
    TokenAddress,
    /// Voting period duration in seconds (set during initialization)
    VotingPeriod,
    /// Total number of registered members (for quorum calculations)
    MemberCount,
    /// Maps member address → premium amount paid
    Member(Address),
    /// Maps claim_id → Claim struct
    Claim(u64),
    /// Maps (claim_id, voter_address) → vote (true = approve)
    Vote(u64, Address),
    /// Maps member address → reputation score
    Reputation(Address),
    /// Total funds available in the insurance pool
    PoolBalance,
    /// Auto-incrementing counter for claim IDs
    ClaimCounter,
}

// ============================================================================
// Errors
// ============================================================================

/// Custom error codes for the MicroInsurancePool contract.
/// Using #[contracterror] enables proper error propagation in Soroban.
#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    /// Member is already registered in the pool
    AlreadyMember = 1,
    /// Address is not a registered pool member
    NotMember = 2,
    /// Premium amount must be greater than zero
    InvalidPremium = 3,
    /// Claim amount must be greater than zero
    InvalidClaimAmount = 4,
    /// The specified claim does not exist
    ClaimNotFound = 5,
    /// Claim is not in a valid state for this operation
    InvalidClaimStatus = 6,
    /// This member has already voted on this claim
    AlreadyVoted = 7,
    /// Pool does not have enough funds to pay the claim
    InsufficientFunds = 8,
    /// Claim amount exceeds the total pool balance
    ClaimExceedsPool = 9,
    /// Contract has already been initialized
    AlreadyInitialized = 10,
    /// Contract has not been initialized yet
    NotInitialized = 11,
    /// Claimant cannot vote on their own claim (conflict of interest)
    SelfVoteNotAllowed = 12,
    /// The voting period has ended; no more votes accepted
    VotingPeriodEnded = 13,
    /// The voting period is still active; cannot execute yet
    VotingPeriodActive = 14,
    /// Minimum voting quorum has not been reached
    QuorumNotReached = 15,
}

// ============================================================================
// Event name constants
// ============================================================================

const EVT_INIT: Symbol = symbol_short!("init");
const EVT_JOIN: Symbol = symbol_short!("join");
const EVT_CLAIM: Symbol = symbol_short!("claim");
const EVT_VOTE: Symbol = symbol_short!("vote");
const EVT_EXEC: Symbol = symbol_short!("exec");

// ============================================================================
// TTL constant for persistent storage
// ============================================================================

/// Number of ledgers to extend TTL on persistent entries.
const PERSISTENT_TTL: u32 = 100;

/// Minimum quorum percentage (>= 50% of members must vote)
const QUORUM_PERCENT: u32 = 50;

// ============================================================================
// Contract
// ============================================================================

#[contract]
pub struct MicroInsurancePool;

#[contractimpl]
impl MicroInsurancePool {
    // ------------------------------------------------------------------------
    // initialize
    // ------------------------------------------------------------------------
    /// Initialize the contract with admin, token address, and voting period.
    /// This must be called once before any other function.
    ///
    /// # Arguments
    /// * `admin` - The admin address that initializes the contract.
    /// * `token` - The SAC token contract address (e.g., native XLM or USDC).
    /// * `voting_period` - Duration in seconds for how long voting stays open.
    ///
    /// # Behavior
    /// - Can only be called once (prevents re-initialization).
    /// - Stores admin, token address, and voting period in persistent storage.
    /// - Emits an "init" event.
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        voting_period: u64,
    ) -> Result<(), ContractError> {
        // Require admin authorization
        admin.require_auth();

        let init_key = DataKey::Initialized;

        // Prevent re-initialization
        if env.storage().persistent().has(&init_key) {
            return Err(ContractError::AlreadyInitialized);
        }

        // Mark as initialized
        env.storage().persistent().set(&init_key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&init_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Store admin address
        let admin_key = DataKey::Admin;
        env.storage().persistent().set(&admin_key, &admin);
        env.storage()
            .persistent()
            .extend_ttl(&admin_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Store token contract address (SAC)
        let token_key = DataKey::TokenAddress;
        env.storage().persistent().set(&token_key, &token);
        env.storage()
            .persistent()
            .extend_ttl(&token_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Store voting period duration
        let vp_key = DataKey::VotingPeriod;
        env.storage().persistent().set(&vp_key, &voting_period);
        env.storage()
            .persistent()
            .extend_ttl(&vp_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Initialize member count to 0
        let mc_key = DataKey::MemberCount;
        env.storage().persistent().set(&mc_key, &0_u32);
        env.storage()
            .persistent()
            .extend_ttl(&mc_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Emit initialization event
        env.events()
            .publish((EVT_INIT, symbol_short!("setup")), (admin, token, voting_period));

        Ok(())
    }

    // ------------------------------------------------------------------------
    // join_pool
    // ------------------------------------------------------------------------
    /// Register a new member in the insurance pool.
    ///
    /// # Arguments
    /// * `member` - The address of the member joining the pool.
    /// * `premium` - The amount of premium tokens the member is paying.
    ///
    /// # Behavior
    /// - Requires authorization from the member address.
    /// - Transfers `premium` tokens from the member to this contract via SAC.
    /// - Validates that the member is not already registered.
    /// - Stores the member's premium and initializes their reputation to 100.
    /// - Increments the member count (used for quorum calculations).
    /// - Emits a "join" event.
    pub fn join_pool(env: Env, member: Address, premium: i128) -> Result<(), ContractError> {
        // Ensure contract is initialized
        Self::require_initialized(&env)?;

        // Require the member to authorize this transaction
        member.require_auth();

        // Cache DataKeys to avoid redundant clones
        let member_key = DataKey::Member(member.clone());
        let rep_key = DataKey::Reputation(member.clone());
        let balance_key = DataKey::PoolBalance;
        let mc_key = DataKey::MemberCount;

        // Check that the member is not already registered
        if env.storage().persistent().has(&member_key) {
            return Err(ContractError::AlreadyMember);
        }

        // Validate premium amount
        if premium <= 0 {
            return Err(ContractError::InvalidPremium);
        }

        // === SAC Token Transfer: member → contract ===
        let token_address = Self::get_token_address(&env)?;
        let token_client = token::Client::new(&env, &token_address);
        let contract_address = env.current_contract_address();
        token_client.transfer(&member, &contract_address, &premium);

        // Store the member's premium contribution
        env.storage().persistent().set(&member_key, &premium);
        env.storage()
            .persistent()
            .extend_ttl(&member_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Initialize reputation score to 100
        env.storage().persistent().set(&rep_key, &100_i128);
        env.storage()
            .persistent()
            .extend_ttl(&rep_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Update the total pool balance
        let current_balance: i128 = env
            .storage()
            .persistent()
            .get(&balance_key)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&balance_key, &(current_balance + premium));
        env.storage()
            .persistent()
            .extend_ttl(&balance_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Increment member count
        let member_count: u32 = env.storage().persistent().get(&mc_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&mc_key, &(member_count + 1));
        env.storage()
            .persistent()
            .extend_ttl(&mc_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Emit a "join" event with the member address and premium amount
        env.events()
            .publish((EVT_JOIN, symbol_short!("member")), (member, premium));

        Ok(())
    }

    // ------------------------------------------------------------------------
    // file_claim
    // ------------------------------------------------------------------------
    /// File an insurance claim as a pool member.
    ///
    /// # Arguments
    /// * `claimant` - The address of the member filing the claim.
    /// * `amount` - The amount being claimed from the pool.
    /// * `reason` - A description of why the claim is being filed.
    ///
    /// # Behavior
    /// - Requires authorization from the claimant.
    /// - Validates that the claimant is a registered member.
    /// - Validates that the claim amount is positive and does not exceed the pool.
    /// - Creates a new claim with status "Pending" and a voting deadline.
    /// - The deadline = current ledger timestamp + voting_period.
    /// - Emits a "claim" event with the claim ID.
    ///
    /// # Returns
    /// The auto-generated claim ID (u64).
    pub fn file_claim(
        env: Env,
        claimant: Address,
        amount: i128,
        reason: String,
    ) -> Result<u64, ContractError> {
        // Ensure contract is initialized
        Self::require_initialized(&env)?;

        // Require the claimant to authorize this transaction
        claimant.require_auth();

        // Cache DataKeys to avoid redundant clones
        let member_key = DataKey::Member(claimant.clone());
        let balance_key = DataKey::PoolBalance;
        let counter_key = DataKey::ClaimCounter;

        // Verify the claimant is a registered pool member
        if !env.storage().persistent().has(&member_key) {
            return Err(ContractError::NotMember);
        }

        // Validate claim amount
        if amount <= 0 {
            return Err(ContractError::InvalidClaimAmount);
        }

        // Ensure claim does not exceed pool balance
        let pool_balance: i128 = env
            .storage()
            .persistent()
            .get(&balance_key)
            .unwrap_or(0);
        if amount > pool_balance {
            return Err(ContractError::ClaimExceedsPool);
        }

        // Generate a new claim ID using an auto-incrementing counter
        let claim_id: u64 = env
            .storage()
            .persistent()
            .get(&counter_key)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&counter_key, &(claim_id + 1));
        env.storage()
            .persistent()
            .extend_ttl(&counter_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Calculate voting deadline = now + voting_period
        let voting_period: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::VotingPeriod)
            .unwrap_or(86400); // default 24 hours
        let deadline = env.ledger().timestamp() + voting_period;

        // Create and store the claim with deadline
        let claim_key = DataKey::Claim(claim_id);
        let claim = Claim {
            claimant: claimant.clone(),
            amount,
            reason,
            votes_approve: 0,
            votes_reject: 0,
            status: ClaimStatus::Pending,
            deadline,
        };
        env.storage().persistent().set(&claim_key, &claim);
        env.storage()
            .persistent()
            .extend_ttl(&claim_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Emit a "claim" event with the claim details
        env.events().publish(
            (EVT_CLAIM, symbol_short!("filed")),
            (claimant, claim_id, amount, deadline),
        );

        Ok(claim_id)
    }

    // ------------------------------------------------------------------------
    // vote_claim
    // ------------------------------------------------------------------------
    /// Vote to approve or reject a pending claim.
    ///
    /// # Arguments
    /// * `voter` - The address of the member casting the vote.
    /// * `claim_id` - The ID of the claim being voted on.
    /// * `approve` - `true` to approve the claim, `false` to reject it.
    ///
    /// # Behavior
    /// - Requires authorization from the voter.
    /// - Validates that the voter is a registered member.
    /// - **Prevents the claimant from voting on their own claim** (conflict of interest).
    /// - **Enforces voting deadline**: votes are rejected after the deadline passes.
    /// - Prevents double voting (each member can only vote once per claim).
    /// - Records the vote and updates the claim's vote counts.
    /// - Emits a "vote" event.
    pub fn vote_claim(
        env: Env,
        voter: Address,
        claim_id: u64,
        approve: bool,
    ) -> Result<(), ContractError> {
        // Ensure contract is initialized
        Self::require_initialized(&env)?;

        // Require the voter to authorize this transaction
        voter.require_auth();

        // Cache DataKeys to avoid redundant clones
        let member_key = DataKey::Member(voter.clone());
        let claim_key = DataKey::Claim(claim_id);
        let vote_key = DataKey::Vote(claim_id, voter.clone());

        // Verify the voter is a registered pool member
        if !env.storage().persistent().has(&member_key) {
            return Err(ContractError::NotMember);
        }

        // Retrieve the claim, ensuring it exists
        let mut claim: Claim = env
            .storage()
            .persistent()
            .get(&claim_key)
            .ok_or(ContractError::ClaimNotFound)?;

        // Only allow voting on pending claims
        if claim.status != ClaimStatus::Pending {
            return Err(ContractError::InvalidClaimStatus);
        }

        // === NEW: Prevent claimant from voting on their own claim ===
        if voter == claim.claimant {
            return Err(ContractError::SelfVoteNotAllowed);
        }

        // === NEW: Enforce voting deadline ===
        if env.ledger().timestamp() > claim.deadline {
            return Err(ContractError::VotingPeriodEnded);
        }

        // Check that this voter hasn't already voted on this claim
        if env.storage().persistent().has(&vote_key) {
            return Err(ContractError::AlreadyVoted);
        }

        // Record the vote
        env.storage().persistent().set(&vote_key, &approve);
        env.storage()
            .persistent()
            .extend_ttl(&vote_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Update vote counts on the claim
        if approve {
            claim.votes_approve += 1;
        } else {
            claim.votes_reject += 1;
        }
        env.storage().persistent().set(&claim_key, &claim);
        env.storage()
            .persistent()
            .extend_ttl(&claim_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Emit a "vote" event
        env.events()
            .publish((EVT_VOTE, symbol_short!("cast")), (voter, claim_id, approve));

        Ok(())
    }

    // ------------------------------------------------------------------------
    // execute_claim
    // ------------------------------------------------------------------------
    /// Execute a claim after the voting period has ended.
    ///
    /// # Arguments
    /// * `claim_id` - The ID of the claim to execute.
    ///
    /// # Behavior
    /// - Retrieves the claim and verifies it is in "Pending" status.
    /// - **Enforces voting deadline**: cannot execute before the deadline.
    /// - **Enforces quorum**: at least 50% of members must have voted.
    /// - Determines the outcome based on majority vote (>50% approve).
    /// - If approved: transfers funds from contract to claimant via SAC token,
    ///   sets status to "Executed", and increases reputation by 5.
    /// - If rejected: keeps funds in pool, sets status to "Rejected",
    ///   and decreases the claimant's reputation by 10.
    /// - Emits an "exec" event with the outcome.
    pub fn execute_claim(env: Env, claim_id: u64) -> Result<(), ContractError> {
        // Ensure contract is initialized
        Self::require_initialized(&env)?;

        // Cache DataKeys to avoid redundant clones
        let claim_key = DataKey::Claim(claim_id);
        let balance_key = DataKey::PoolBalance;

        // Retrieve the claim
        let mut claim: Claim = env
            .storage()
            .persistent()
            .get(&claim_key)
            .ok_or(ContractError::ClaimNotFound)?;

        // Only execute claims that are still pending
        if claim.status != ClaimStatus::Pending {
            return Err(ContractError::InvalidClaimStatus);
        }

        // === NEW: Enforce voting deadline — cannot execute before deadline ===
        if env.ledger().timestamp() <= claim.deadline {
            return Err(ContractError::VotingPeriodActive);
        }

        // === NEW: Enforce minimum quorum ===
        let member_count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::MemberCount)
            .unwrap_or(0);
        let total_votes = claim.votes_approve + claim.votes_reject;
        // Quorum: at least QUORUM_PERCENT% of members (excluding claimant) must vote
        let eligible_voters = if member_count > 0 {
            member_count - 1 // Claimant cannot vote, so subtract 1
        } else {
            0
        };
        let min_votes = (eligible_voters * QUORUM_PERCENT) / 100;
        if total_votes < min_votes {
            return Err(ContractError::QuorumNotReached);
        }

        // Cache reputation key (needs claimant, so must come after claim read)
        let rep_key = DataKey::Reputation(claim.claimant.clone());

        // Read pool balance ONCE — reused in both branches
        let pool_balance: i128 = env
            .storage()
            .persistent()
            .get(&balance_key)
            .unwrap_or(0);

        // Determine outcome: majority (>50%) approves
        if total_votes > 0 && claim.votes_approve > claim.votes_reject {
            // === APPROVED ===

            // Ensure pool has sufficient funds
            if claim.amount > pool_balance {
                return Err(ContractError::InsufficientFunds);
            }

            // === SAC Token Transfer: contract → claimant ===
            let token_address = Self::get_token_address(&env)?;
            let token_client = token::Client::new(&env, &token_address);
            let contract_address = env.current_contract_address();
            token_client.transfer(&contract_address, &claim.claimant, &claim.amount);

            // Deduct from pool balance
            env.storage()
                .persistent()
                .set(&balance_key, &(pool_balance - claim.amount));
            env.storage()
                .persistent()
                .extend_ttl(&balance_key, PERSISTENT_TTL, PERSISTENT_TTL);

            // Mark claim as executed (approved)
            claim.status = ClaimStatus::Executed;
            env.storage().persistent().set(&claim_key, &claim);
            env.storage()
                .persistent()
                .extend_ttl(&claim_key, PERSISTENT_TTL, PERSISTENT_TTL);

            // Increase claimant reputation by 5 for an approved claim
            let rep: i128 = env
                .storage()
                .persistent()
                .get(&rep_key)
                .unwrap_or(100);
            env.storage()
                .persistent()
                .set(&rep_key, &(rep + 5));
            env.storage()
                .persistent()
                .extend_ttl(&rep_key, PERSISTENT_TTL, PERSISTENT_TTL);

            // Emit approved event
            env.events().publish(
                (EVT_EXEC, symbol_short!("approved")),
                (claim.claimant, claim_id, claim.amount),
            );
        } else {
            // === REJECTED ===
            claim.status = ClaimStatus::Rejected;
            env.storage().persistent().set(&claim_key, &claim);
            env.storage()
                .persistent()
                .extend_ttl(&claim_key, PERSISTENT_TTL, PERSISTENT_TTL);

            // Decrease claimant reputation by 10 for a rejected claim
            let rep: i128 = env
                .storage()
                .persistent()
                .get(&rep_key)
                .unwrap_or(100);
            env.storage()
                .persistent()
                .set(&rep_key, &(rep - 10));
            env.storage()
                .persistent()
                .extend_ttl(&rep_key, PERSISTENT_TTL, PERSISTENT_TTL);

            // Emit rejected event
            env.events().publish(
                (EVT_EXEC, symbol_short!("rejected")),
                (claim.claimant, claim_id),
            );
        }

        Ok(())
    }

    // ------------------------------------------------------------------------
    // get_reputation
    // ------------------------------------------------------------------------
    /// Get the reputation score for a pool member.
    ///
    /// # Arguments
    /// * `member` - The address of the member to query.
    ///
    /// # Returns
    /// The member's reputation score. New members start at 100.
    /// - Score increases by 5 when a filed claim is approved.
    /// - Score decreases by 10 when a filed claim is rejected.
    pub fn get_reputation(env: Env, member: Address) -> i128 {
        let rep_key = DataKey::Reputation(member);
        env.storage()
            .persistent()
            .get(&rep_key)
            .unwrap_or(0)
    }

    // ------------------------------------------------------------------------
    // Helper: get_pool_balance (read-only)
    // ------------------------------------------------------------------------
    /// Get the current total balance of the insurance pool.
    pub fn get_pool_balance(env: Env) -> i128 {
        let balance_key = DataKey::PoolBalance;
        env.storage()
            .persistent()
            .get(&balance_key)
            .unwrap_or(0)
    }

    // ------------------------------------------------------------------------
    // Helper: get_claim (read-only)
    // ------------------------------------------------------------------------
    /// Retrieve a specific claim by its ID.
    pub fn get_claim(env: Env, claim_id: u64) -> Result<Claim, ContractError> {
        let claim_key = DataKey::Claim(claim_id);
        env.storage()
            .persistent()
            .get(&claim_key)
            .ok_or(ContractError::ClaimNotFound)
    }

    // ------------------------------------------------------------------------
    // Helper: get_member_count (read-only)
    // ------------------------------------------------------------------------
    /// Get the total number of registered pool members.
    pub fn get_member_count(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::MemberCount)
            .unwrap_or(0)
    }

    // ------------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------------

    /// Ensure the contract has been initialized before use.
    fn require_initialized(env: &Env) -> Result<(), ContractError> {
        if !env.storage().persistent().has(&DataKey::Initialized) {
            return Err(ContractError::NotInitialized);
        }
        Ok(())
    }

    /// Retrieve the stored SAC token address.
    fn get_token_address(env: &Env) -> Result<Address, ContractError> {
        env.storage()
            .persistent()
            .get(&DataKey::TokenAddress)
            .ok_or(ContractError::NotInitialized)
    }
}

mod test;
