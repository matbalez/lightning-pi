# Lightning pi

A small, real-money demonstration of **x402 v2 over Bitcoin Lightning**, backed by a dedicated Lexe wallet.

**Live:** https://lightning-pi-matbalez.fly.dev/  
**Agent skill:** https://lightning-pi-matbalez.fly.dev/SKILL.md

`GET /digits-of-pi?digits=100` costs **₿100** plus payer routing fees. Integers from 0 through 10,000 are supported. The response contains pi as a string, correctly rounded to the requested decimal places. Invalid requests are rejected before an invoice is created.

```json
{"digits":3,"pi":"3.142","rounding":"nearest"}
```

## Try it without paying

```sh
curl -i 'https://lightning-pi-matbalez.fly.dev/digits-of-pi?digits=3'
```

The HTTP 402 response carries the canonical base64 JSON `PAYMENT-REQUIRED` header. Its JSON body mirrors that challenge for inspection. Each unpaid call creates a fresh BOLT11 invoice. This is a live mainnet service.

**Save the entire selected `accepts` entry and exact URL before paying.** The paid retry's `accepted` object must preserve that entry, including its original invoice. Fetching another challenge gives you a different invoice, which cannot be paired with the first payment's preimage. The original proof remains redeemable with its saved challenge until expiry/grace; never pay again automatically to repair a header.

## Pay with an agent

Give the agent the [SKILL.md](skills/x402-lightning/SKILL.md) and access to its own funded Lightning wallet. That wallet must return a payment preimage. The server never needs the payer's credentials.

For agents scripting HTTP directly, the skill includes a [manual Python handshake](skills/x402-lightning/SKILL.md#manual-http-handshake-in-python): persist the challenge, validate/pay its invoice with your wallet, then construct the exact `PAYMENT-SIGNATURE` envelope from the saved entry. These transport snippets complement the required invoice validation; the Rust client below implements that validation.

The reference client uses the same strict invoice and request validation as the service:

```sh
git clone https://github.com/matbalez/lightning-pi.git
cd lightning-pi
cargo build --locked --release --bin x402-client

# Use YOUR payer credential file, with spend/read_info/read_payments scopes.
export LEXE_CLIENT_CREDENTIALS_PATH=/absolute/private/payer-credentials.txt

target/release/x402-client --state /absolute/private/pi-purchase.json buy \
  --url 'https://lightning-pi-matbalez.fly.dev/digits-of-pi?digits=3' \
  --max-amount-msat 100000 --max-fee-msat 10000
```

The default amount limit is ₿100; the default routing fee limit is ₿10. The client preflights the Lexe route, requires the exact invoice face value, and submits the returned route continuation. It writes a private journal before initiating payment and before redemption. Keep that file: it contains the bearer payment proof. A second `buy` using the same file refuses to make another purchase.

Interrupted payment:

```sh
target/release/x402-client --state /absolute/private/pi-purchase.json recover-lexe
target/release/x402-client --state /absolute/private/pi-purchase.json redeem
```

Recovery looks up the original payment; it never sends a new one. If the client already saved a successful result, `redeem` returns that local result. If the server consumed a proof but its response was lost, a network retry gets `duplicate_settlement`; the demo has no server-side paid-response recovery.

### Other wallets, including MDK

```sh
target/release/x402-client --state /absolute/private/pi-purchase.json prepare \
  --url 'https://lightning-pi-matbalez.fly.dev/digits-of-pi?digits=3'
```

This prints a validated invoice without spending. Have your wallet pay that exact invoice and return this adapter result to a private JSON file:

```json
{
  "status": "paid",
  "invoice": "the exact original BOLT11 invoice",
  "paymentHash": "64 lowercase hex characters",
  "amountMsat": "100000",
  "feeMsat": "actual routing fee as an integer decimal string",
  "preimage": "64 lowercase hex characters"
}
```

`amountMsat` excludes routing fees. Populate the invoice, payment hash and amount from the completed wallet payment record, not from an unverified assertion. Pending/failed payments must not report `paid`.

```sh
target/release/x402-client --state /absolute/private/pi-purchase.json attach-result \
  --payment-result /absolute/private/wallet-result.json
target/release/x402-client --state /absolute/private/pi-purchase.json redeem
```

Money Dev Kit agent-wallet **0.22.0** exposes payment preimages in completed payment records (verified against the published package). After `prepare`, use your funded MDK wallet to pay the validated invoice, then import its payment history:

```sh
umask 077
npx @moneydevkit/agent-wallet@0.22.0 send 'THE_VALIDATED_INVOICE' > /absolute/private/mdk-send.json
npx @moneydevkit/agent-wallet@0.22.0 payments > /absolute/private/mdk-payments.json
target/release/x402-client --state /absolute/private/pi-purchase.json attach-mdk \
  --payments-file /absolute/private/mdk-payments.json
target/release/x402-client --state /absolute/private/pi-purchase.json redeem
```

If `send` times out, use `payments` to inspect the original payment; never repeat `send` automatically. The adapter requires a completed outbound record whose destination is the original invoice, checks amount and payment hash, and verifies the preimage. Follow your MDK wallet's spending/fee policy; this import command does not initiate or cap wallet spending. The CLI import path is tested with the published record schema; an MDK-funded live payment has not been run. Lexe is the live-tested payer. [MDK docs](https://docs.moneydevkit.com/agent-wallet), [payment record source](https://github.com/moneydevkit/mdk-checkout/blob/main/packages/agent-wallet/src/payment-store.ts).

## Architecture

```mermaid
sequenceDiagram
    participant A as Agent
    participant S as Pi service
    participant W as Agent wallet
    participant L as Lexe receiver
    participant F as Private facilitator
    A->>S: GET /digits-of-pi?digits=N
    S->>L: Create invoice with signed request hash
    L-->>S: BOLT11 invoice
    S-->>A: 402 + PAYMENT-REQUIRED
    A->>A: Validate invoice and exact request binding
    A->>W: Pay invoice
    W->>L: Lightning payment
    W-->>A: Completed payment and preimage
    A->>S: Same GET + PAYMENT-SIGNATURE
    S->>F: POST /settle, expected terms derived locally
    F->>F: Validate signature/proof; atomically record use
    F-->>S: Settlement receipt
    S-->>A: 200 + decimal pi + PAYMENT-RESPONSE
```

- Scheme: `exact`; asset: `BTC`; transfer method: `bolt11`; flow: `upfront`.
- Mainnet network: `lnbtc:000000000019d6689c085ae165831e93`. This implementation deliberately supports mainnet and the HTTP profile only.
- Wire amounts are positive integer **millisatoshi strings**, so ₿100 is `"100000"`.
- The binding is SHA-256 of RFC 8785 canonical JSON containing domain, GET method, exact public URL, empty body hash and empty bound-header list. This endpoint always returns JSON and has no account selection, content negotiation or header-dependent operation. It rejects request bodies and content encoding.
- Public origin is configured and Host is validated. Forwarded headers cannot override the binding. The exact query bytes are retained.
- Each invoice expires after 300 seconds. Paid redemption gets the spec's 60-second grace period. The original accepted invoice is used on retry; all other authoritative terms are reconstructed.
- The facilitator runs on **127.0.0.1:8081**, outside Fly's public service port, and never queries the receiving node. It verifies the preimage locally and stores `network:payment_hash` atomically in SQLite with WAL and `synchronous=FULL`.
- Entries remain until more than one hour past invoice expiry plus grace. The Fly volume preserves them through process and machine restarts. Keep **one machine** with this volume. Multiple machines require a shared strongly consistent settlement database. Do not restore a stale snapshot while older proofs can still be valid.
- Requests are executed only after settlement succeeds. Concurrent use of a proof yields one result and one `duplicate_settlement` error. The service uses HTTP 409 for that error.
- Paid failure responses retain the specification's stable error codes and settlement receipt, with additional `message` and `hint` fields for common mistakes. A preimage/invoice mismatch and a recipient-field mismatch receive different explanations. Validation failures do not consume the proof or create a replacement invoice.
- Invoice creation is limited to four concurrent calls and 120 challenges per minute globally; paid work has a separate concurrency limit.
- Pi uses integer interval arithmetic and Machin's formula. It checks that both error bounds round identically, avoiding floating-point precision loss.

Lightning payment happens before HTTP delivery. Lost responses and server failures after settlement can therefore leave a buyer paid without a result. This demonstration provides no automatic refund or response recovery. Overpayment buys no additional credit. The receiver's net proceeds may be smaller than the invoice because Lexe applies receiving fees.

## Lexe integration detail

Lexe's stable `CreateInvoiceRequest` in version **0.1.23** does not expose `description_hash` and sets it to `None`. The underlying node API supports it. `src/wallet.rs` uses the SDK's `unstable` feature and `UserNodeRunApi::create_invoice` to supply the required 32-byte hash. Lexe versions and Cargo.lock are pinned. This path has been exercised against a real Lexe mainnet wallet.

Only receiving/read credentials go to the deployed service. Root seeds and the test payer's spending credentials are kept outside the repository and outside the Docker build context. Invoice-issuance authority for the receiver must remain restricted to this service and trusted wallet owners.

## Run and deploy

Requirements: Rust 1.95, a provisioned Lexe mainnet wallet, and a client credential with `receive`, `read_info` and `read_payments` scopes.

```sh
export LEXE_CLIENT_CREDENTIALS_PATH=/absolute/private/receiver-credentials.txt
export PUBLIC_ORIGIN=http://127.0.0.1:8080
cargo run --locked --bin lightning-pi
```

Configuration: `PUBLIC_ORIGIN` is required; `PRICE_MSAT=100000`, `MAX_DIGITS=10000`, `REPLAY_DB=data/replay.sqlite`, `LISTEN_ADDR=0.0.0.0:8080` are defaults. The configured price must be a whole bitcoin base-unit amount. Adjust the hosted skill's price and limits if changing these settings.

The included Fly configuration uses a persistent volume and a single machine in `sjc`. For a new deployment, change the app name and public origin in `fly.toml`, the hosted skill and service metadata, then:

```sh
fly apps create YOUR_APP --org YOUR_ORG
fly volumes create replay_data --app YOUR_APP --region sjc --size 1
# Stage LEXE_CLIENT_CREDENTIALS through Fly secrets, never a checked-in file.
fly deploy --remote-only --ha=false
```

For this deployment the app and volume already exist. Use `fly deploy --remote-only --ha=false` from this repository to update it. `/healthz` is a free process-health endpoint; a successful unpaid 402 request additionally checks live invoice creation. Wallet funding, credentials and root-seed recovery use [Lexe's documented tools](https://docs.lexe.tech/authentication/).

## Validation and interoperability note

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Tests cover decimal rounding through 10,000 places, request-binding hashes, strict invoice validation, tampering, expiry/skew boundaries, duplicate JSON keys, HTTP payment flow, concurrent redemption, and replay protection after reopening the database.

The HTTP integration test also executes the Python snippets directly from the skill (Python 3 required), verifies that challenge files cannot be overwritten, and reproduces mixing invoice A's proof with challenge B. Failed validation leaves the original proof usable, with no new invoice created.

Live mainnet verification on September 23, 2026: a separate Lexe wallet paid for 10 and 10,000 decimal places, and both returned successful x402 receipts. The 10,000-place result matched an independent Chudnovsky calculation. Replaying its proof after a Fly machine restart returned `409 duplicate_settlement`, confirming that consumption survived the restart. The hosted skill was checked byte for byte against this repository.

The invoice in the merged x402 spec's original positive fixture omits the BOLT11 feature field. LDK 0.35.0-beta1 rejects it as `InvalidFeatures`. We retain it unchanged under `tests/fixtures` and explicitly test that incompatibility. Positive settlement tests use the same request, signer, amount, preimage and timestamps with a newly signed invoice including the feature flags. The published request-binding hashes match exactly. Production Lexe invoices include features and pass strict LDK validation. We do not weaken invoice checks to accept the example.

Sources: [merged PR #2861](https://github.com/x402-foundation/x402/pull/2861), [Lightning scheme](https://github.com/x402-foundation/x402/blob/main/specs/schemes/exact/scheme_exact_lnbtc.md), [HTTP transport](https://github.com/x402-foundation/x402/blob/main/specs/transports-v2/http.md), [Lexe SDK source](https://github.com/lexe-app/lexe-public/tree/6344b122735cb4f5a552148c16b814872c91588a/lexe).
