# MicroInsurancePool — Soroban Smart Contract

A decentralized insurance pool on Stellar where members pay premiums, file claims, vote on each other's payouts, and receive real token transfers. No company, no middleman — just code.

---

## What It Does

1. **Members join** by paying a premium → real SAC token transfer to the contract
2. **Claims are filed** by members who need a payout → voting window opens automatically
3. **Community votes** to approve or reject within the deadline
4. **Approved claims** → contract transfers tokens directly to claimant
5. **Rejected claims** → funds stay in pool, claimant reputation decreases
6. **Reputation scores** track member trustworthiness over time

---

## Contract Functions

| Function | Description |
|---|---|
| `initialize(admin, token, voting_period)` | One-time setup: SAC token address + voting duration (seconds) |
| `join_pool(member, premium)` | Register + real SAC token payment into pool |
| `file_claim(claimant, amount, reason)` | File claim, generates deadline timestamp |
| `vote_claim(voter, claim_id, approve)` | Vote (deadline enforced, no self-voting) |
| `execute_claim(claim_id)` | Finalize after deadline + quorum; triggers token payout |
| `get_reputation(member)` | Query reputation score |
| `get_pool_balance()` | Total funds in pool |
| `get_claim(claim_id)` | Get claim details |
| `get_member_count()` | Total registered members |

---

## Governance Rules

| Rule | Value |
|---|---|
| Voting quorum | ≥ 50% of eligible members must vote |
| Majority threshold | > 50% approve → claim paid |
| Voting deadline | Configurable (set during `initialize`) |
| Self-voting | ❌ Not allowed (conflict of interest) |
| Reputation change | +5 approved, −10 rejected |

---

## Build

```bash
# Run all 10 tests
cargo test -p micro-insurance-pool

# Build WASM
cargo build --target wasm32-unknown-unknown --release -p micro-insurance-pool
```

---

## Deploy to Stellar Testnet

```bash
# Fund deployer account
stellar keys generate --global deployer --network testnet
stellar keys fund deployer --network testnet

# Deploy contract
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/micro_insurance_pool.wasm \
  --source deployer \
  --network testnet

# Initialize (native XLM SAC, 1-hour voting)
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source deployer \
  --network testnet \
  -- initialize \
  --admin <YOUR_ADDRESS> \
  --token <SAC_TOKEN_ADDRESS> \
  --voting_period 3600
```

---

## Example Interactions

```bash
# Join the pool with 100 XLM (10,000,000 stroops)
stellar contract invoke --id <CONTRACT_ID> --source member1 --network testnet \
  -- join_pool --member <MEMBER_ADDRESS> --premium 10000000

# File a claim
stellar contract invoke --id <CONTRACT_ID> --source member1 --network testnet \
  -- file_claim --claimant <MEMBER_ADDRESS> --amount 5000000 --reason "Medical emergency"

# Vote to approve claim #0
stellar contract invoke --id <CONTRACT_ID> --source member2 --network testnet \
  -- vote_claim --voter <VOTER_ADDRESS> --claim_id 0 --approve true

# Execute claim after voting ends
stellar contract invoke --id <CONTRACT_ID> --source deployer --network testnet \
  -- execute_claim --claim_id 0
```

---

## License

MIT
