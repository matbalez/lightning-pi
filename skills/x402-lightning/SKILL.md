---
name: x402-lightning
description: Purchase digits of pi from the Lightning pi x402 v2 endpoint using a Lightning wallet that returns payment preimages. Use for requesting paid pi results or integrating a payer with this service.
---

# Lightning pi

Service: https://lightning-pi-matbalez.fly.dev

Request `GET /digits-of-pi?digits=N`, where N is a canonical integer from 0 through 10000. Price: ₿100 per request plus payer routing fees. JSON contains `pi` as a string, correctly rounded to N decimal places.

Use within the user's payment authorization. Your wallet must be funded on Bitcoin mainnet and return the 32-byte payment preimage after successful payment. A payment hash or `paid` flag alone is insufficient. Never give the service your wallet credentials.

Use the client helper in https://github.com/matbalez/lightning-pi when possible. It validates invoices before payment and preserves state for recovery. See that repository's client instructions for commands and the wallet adapter contract.

## Reference client

Clone the repository and run `cargo build --locked --release --bin x402-client` (Rust 1.95). Use a private state file outside the checkout.

For Lexe, set `LEXE_CLIENT_CREDENTIALS_PATH` to your payer credential file (spend/read_info/read_payments scopes), then:

```sh
target/release/x402-client --state /absolute/private/pi.json buy \
  --url 'https://lightning-pi-matbalez.fly.dev/digits-of-pi?digits=100' \
  --max-amount-msat 100000 --max-fee-msat 10000
```

For Money Dev Kit agent-wallet 0.22.0, first run the helper's `prepare --url URL` subcommand. Pay only the validated invoice using `npx @moneydevkit/agent-wallet@0.22.0 send INVOICE`, saving output privately. Use `payments` to save the completed payment history to a private JSON file. Run the helper's `attach-mdk --payments-file FILE`, then `redeem`, always with the same `--state` path before the subcommand. The importer verifies the record's destination, completed status, amount, hash and preimage. It does not initiate wallet spending or enforce MDK fee limits. The published MDK record schema has been tested; the live payment test uses Lexe.

On an interrupted Lexe payment, run `recover-lexe` (read-only payment lookup), then `redeem`. On a lost redemption response, run `redeem` again. Never run another `buy` automatically. Do not expose state files, payment history or preimages in chat or public logs.

## Protocol

1. Send the intended GET over HTTPS with no body and redirects disabled. Save the exact URL, including query string.
2. Base64-decode the HTTP 402 `PAYMENT-REQUIRED` header as JSON. Require `x402Version: 2` and `resource.url` equal to your exact URL. Select `scheme: exact`, `network: lnbtc:000000000019d6689c085ae165831e93`, `asset: BTC`, `extra.assetTransferMethod: bolt11` (or omitted), and `extra.paymentFlow: upfront`. `amount` is an integer decimal string in millisatoshis: the service price is `"100000"`. Check your spending limit.
3. Strictly validate the BOLT11 signature, amount, network, receiver key (`payTo`), creation time and expiry. Require `extra.requestBindingProfile: http:1` and `extra.requestBindingParams: {"headers":[]}`. Construct the object below locally, serialize using RFC 8785 JCS, then SHA-256 its UTF-8 bytes. Require both `extra.requestHash` and the invoice's single signed description hash (`h`) to equal this digest. Reject inline descriptions (`d`), unsupported profiles and missing binding fields.

```json
{"bodyHash":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","domain":"x402:exact:lnbtc:bolt11:http:1","headers":[],"method":"GET","url":"THE EXACT REQUEST URL"}
```

4. Pay the original `extra.invoice` exactly, using your own wallet. Require a completed payment for the same invoice, payment hash and face amount, excluding routing fees. Verify `SHA256(hex_decode(preimage))` equals the invoice payment hash. The preimage must be 64 lowercase hex characters. Keep it private until redemption.
5. Retry the identical GET with `PAYMENT-SIGNATURE: BASE64(JSON)` containing `{"x402Version":2,"accepted":ORIGINAL_ACCEPTS_ENTRY,"payload":{"preimage":"..."}}`. Preserve the original invoice byte for byte. Do not use L402 authorization headers or call `/verify`.
6. On HTTP 200, validate the base64 JSON `PAYMENT-RESPONSE` header: success must be true, network must match, and transaction must equal the payment hash. Return the pi string without floating-point conversion.

## Failures and retries

- Invalid digit counts fail before payment. Fix them before purchasing.
- If payment is pending or the wallet times out, look up that exact payment. Never automatically start a replacement payment.
- Preserve the original challenge and proof on connection loss. Retry the same proof only for the exact original request. Never repay to recover a lost response automatically.
- Proofs are single use. `duplicate_settlement` (HTTP 409) means a previous attempt consumed the proof, potentially before a response was lost. Stop and report this; the demo has no paid-response recovery or automatic refund.
- Redemption is allowed through invoice expiry plus 60 seconds. After that, report the failure. Lightning payment precedes delivery; overpayment buys no extra credit and no protocol refund is provided.
- This skill does not make every x402 library Lightning-compatible. MDK's automatic L402 handling uses different headers; use the explicit x402 flow above.

Specification: https://github.com/x402-foundation/x402/blob/main/specs/schemes/exact/scheme_exact_lnbtc.md
