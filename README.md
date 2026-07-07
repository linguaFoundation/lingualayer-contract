# LinguaLayer — Language Rights Protocol on Stellar

> **Your language. Your data. Your share.**

LinguaLayer builds transparent licensing and royalty splits for African and underrepresented languages — so communities earn when their voices power the next wave of AI.

[![CI](https://github.com/linguaFoundation/lingualayer-contract/actions/workflows/ci.yml/badge.svg)](https://github.com/linguaFoundation/lingualayer-contract/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Stellar Testnet](https://img.shields.io/badge/Stellar-Testnet-blue)](https://stellar.org)

---

## 🆕 What's New in v2 (Appeal Update)

### 1. QualityOracle Contract
Trusted language curators stake XLM and submit quality attestations (0–100) for every dataset. Scores drive a **royalty multiplier**:

| Tier | Score | Royalty Multiplier |
|---|---|---|
| Platinum | 85–100 | **1.5×** |
| Gold | 70–84 | **1.25×** |
| Silver | 40–69 | **1.0×** |
| Bronze | 1–39 | **0.75×** |

This creates a marketplace incentive for contributors to produce high-quality data.

### 2. Dataset Commissioning Escrow (DataCommission Contract)
AI companies can post **USDC bounties** for specific language datasets:
- Post a commission → USDC locked in smart contract escrow
- Contributors deliver → admin verifies → escrow released on-chain
- Cancellable after deadline with full refund
- Browseable at `/bounties` — our new **Language Bounty Board** page

### 3. All Stellar Wallets via Stellar Wallets Kit
We replaced our custom Freighter-only integration with **[@creit-tech/stellar-wallets-kit](https://github.com/Creit-Tech/Stellar-Wallets-Kit)** — the standard multi-wallet library:

| Wallet | Type | Status |
|---|---|---|
| Freighter | Browser Extension | ✅ |
| xBull | Extension + Mobile | ✅ |
| Lobstr | Extension + Mobile | ✅ |
| Hana Wallet | Extension | ✅ |
| Rabet | Extension | ✅ |
| WalletConnect | Mobile QR | ✅ |
| Ledger | Hardware | ✅ |
| ALBEDO | Web | ✅ |

### 4. SEP-0010 Web Authentication
Replaced custom SIWS with the **official Stellar SEP-0010** standard — recognized by all Stellar wallets and compliant with the Stellar ecosystem authentication spec.

### 5. Contributor Reputation System
On-chain reputation scores (0–1000) updated automatically when contributors register datasets. Higher reputation = higher platform visibility and governance weight.

---

## Architecture

```
linguaFoundation/
├── lingualayer-contract/     # Soroban smart contracts (Rust)
│   ├── dataset-registry/     # Dataset + contributor + reputation
│   ├── license-router/       # License issuance and validation
│   ├── royalty-splitter/     # Revenue distribution (SAC/USDC)
│   ├── quality-oracle/       # NEW: on-chain quality attestations
│   └── data-commission/      # NEW: USDC escrow for data bounties
├── lingualayer-backend/      # Fastify API + Soroban indexer (TypeScript)
└── lingualayer-frontend/     # Next.js 14 app (TypeScript)
```

## Deployed Contracts (Testnet)

| Contract | Address |
|---|---|
| DatasetRegistry v3 | `CDATASET...` |
| LicenseRouter v2 | `CLICENSE...` |
| RoyaltySplitter v2 | `CROYALTY...` |
| QualityOracle v1 | `CQUALITY...` |
| DataCommission v1 | `CCOMMISS...` |

> Redeploy with: `stellar contract deploy --wasm target/wasm32-unknown-unknown/release/<name>.wasm --network testnet`

## Local Development

```bash
# Contracts
cd lingualayer-contract
cargo build --target wasm32-unknown-unknown --release

# Backend
cd lingualayer-backend
cp .env.example .env
npm install && npm run dev

# Frontend
cd lingualayer-frontend
npm install && npm run dev
```

## Contributing

We welcome contributions from linguists, developers, and community members.
Please read [CONTRIBUTING.md](CONTRIBUTING.md) and open an issue before submitting a PR.

## License

MIT © linguaFoundation
