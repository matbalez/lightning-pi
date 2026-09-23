---
name: x402-lightning
description: Purchase digits of pi from the Lightning pi x402 v2 endpoint using a Lightning wallet that returns payment preimages. Use for requesting paid pi results or integrating a payer with this service.
---

# Lightning pi

Service: https://lightning-pi-matbalez.fly.dev

Request `GET /digits-of-pi?digits=N`, where N is a canonical integer from 0 through 10000. Price: ₿100 per request plus payer routing fees. JSON contains `pi` as a string, correctly rounded to N decimal places.

Use within the user's payment authorization. Your wallet must be funded on Bitcoin mainnet and return the 32-byte payment preimage after successful payment. A payment hash or `paid` flag alone is insufficient. Never give the service your wallet credentials.

Use the client helper in https://github.com/matbalez/lightning-pi when possible. It validates invoices before payment and preserves state for recovery. See that repository's client instructions for commands and the wallet adapter contract.

## Save the original challenge before paying

Every unpaid GET to this service creates a **new invoice**, even for the same URL. Before paying, save the exact URL and the entire selected `accepts` entry to a private file. In `PAYMENT-SIGNATURE`, `accepted` must be that original entry from the challenge whose invoice you paid, with all values preserved. It is one object, not the `accepts` array or the whole `PAYMENT-REQUIRED` response. JSON key order and whitespace may change; the invoice string and field values must not.

**Do not fetch another challenge to reconstruct the header after paying.** A new challenge B cannot be paired with the preimage for invoice A. Fetching B does not invalidate A: restore A's saved entry and retry the original URL while A is still redeemable. Never pay a replacement invoice automatically. If the original entry is lost, stop and report that instead of substituting a new one.

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
2. Base64-decode the HTTP 402 `PAYMENT-REQUIRED` header as JSON. Require `x402Version: 2` and `resource.url` equal to your exact URL. Select `scheme: exact`, `network: lnbtc:000000000019d6689c085ae165831e93`, `asset: BTC`, `extra.assetTransferMethod: bolt11` (or omitted), and `extra.paymentFlow: upfront`. `amount` is an integer decimal string in millisatoshis: the service price is `"100000"`. Check your spending limit. Persist the entire selected entry and URL before paying; never replace this saved challenge on a retry.
3. Strictly validate the BOLT11 signature, amount, network, receiver key (`payTo`), creation time and expiry. Require `extra.requestBindingProfile: http:1` and `extra.requestBindingParams: {"headers":[]}`. Construct the object below locally, serialize using RFC 8785 JCS, then SHA-256 its UTF-8 bytes. Require both `extra.requestHash` and the invoice's single signed description hash (`h`) to equal this digest. Reject inline descriptions (`d`), unsupported profiles and missing binding fields.

```json
{"bodyHash":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","domain":"x402:exact:lnbtc:bolt11:http:1","headers":[],"method":"GET","url":"THE EXACT REQUEST URL"}
```

4. Pay the original `extra.invoice` exactly, using your own wallet. Require a completed payment for the same invoice, payment hash and face amount, excluding routing fees. Verify `SHA256(hex_decode(preimage))` equals the invoice payment hash. The preimage must be 64 lowercase hex characters. Keep it private until redemption.
5. Retry the identical GET with `PAYMENT-SIGNATURE: BASE64(JSON)` containing `{"x402Version":2,"accepted":ORIGINAL_ACCEPTS_ENTRY,"payload":{"preimage":"..."}}`. Preserve the original invoice byte for byte. Do not use L402 authorization headers or call `/verify`.
6. On HTTP 200, validate the base64 JSON `PAYMENT-RESPONSE` header: success must be true, network must match, and transaction must equal the payment hash. Return the pi string without floating-point conversion.

## Manual HTTP handshake in Python

These standard-library snippets handle transport and persistence. They do **not** implement BOLT11 signature/expiry or request-binding validation: perform the checks in protocol step 3 before paying, using a capable invoice library or the reference client. Use a new private directory outside your repository for each purchase, and run both snippets from that same directory. Keep `challenge.json` and the wallet result; do not paste their contents into chat.

First, fetch and save one challenge. This creates no payment and refuses to overwrite an existing saved challenge:

```python
import base64, json, os
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import build_opener, HTTPRedirectHandler

class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None

url = "https://lightning-pi-matbalez.fly.dev/digits-of-pi?digits=25"
if Path("challenge.json").exists():
    raise SystemExit("Reuse challenge.json; do not fetch a replacement.")
http = build_opener(NoRedirect)
try:
    response = http.open(url, timeout=30)
except HTTPError as exc:
    response = exc  # urllib treats HTTP 402 as an exception.
with response:
    if response.status != 402:
        raise SystemExit(f"Expected 402, got {response.status}")
    challenge = json.loads(base64.b64decode(response.headers["PAYMENT-REQUIRED"], validate=True))
if challenge["x402Version"] != 2 or challenge["resource"]["url"] != url:
    raise SystemExit("Challenge version or URL mismatch")
accepted = next(a for a in challenge["accepts"]
                if a["scheme"] == "exact" and a["network"] == "lnbtc:000000000019d6689c085ae165831e93")
fd = os.open("challenge.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w") as f:
    json.dump({"url": url, "accepted": accepted}, f)
    f.flush()
    os.fsync(f.fileno())
print("Saved challenge.json. Validate its invoice and request binding before paying.")
```

Validate, then pay **only** the saved `accepted.extra.invoice`. Save the completed wallet record privately as `wallet-result.json`, using this adapter shape (amount excludes fees; invoice/hash/amount must come from the verified completed payment):

```json
{
  "status": "paid",
  "invoice": "THE_EXACT_PAID_BOLT11_INVOICE",
  "amountMsat": "100000",
  "paymentHash": "64 lowercase hex characters from the paid invoice",
  "preimage": "64 lowercase hex characters from the completed payment"
}
```

Then redeem once. This reads the original entry from disk and makes **no new challenge request**. It blocks redirects, checks the wallet record, and validates the settlement receipt:

```python
import base64, hashlib, json, re
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, build_opener, HTTPRedirectHandler

class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None

saved = json.loads(Path("challenge.json").read_text())
paid = json.loads(Path("wallet-result.json").read_text())
accepted = saved["accepted"]
if (paid["status"] != "paid" or paid["invoice"] != accepted["extra"]["invoice"]
        or paid["amountMsat"] != accepted["amount"]):
    raise SystemExit("Wallet payment does not match the saved challenge; do not pay again.")
preimage = paid["preimage"]
if not isinstance(preimage, str) or not re.fullmatch(r"[0-9a-f]{64}", preimage):
    raise SystemExit("Expected the completed payment's 64-character lowercase hex preimage")
payment_hash = hashlib.sha256(bytes.fromhex(preimage)).hexdigest()
if payment_hash != paid["paymentHash"]:
    raise SystemExit("Preimage does not match the completed payment hash")

signature_obj = {
    "x402Version": 2,
    "accepted": accepted,  # Entire ORIGINAL accepts entry, including its invoice.
    "payload": {"preimage": preimage},
}
signature = base64.b64encode(json.dumps(signature_obj, separators=(",", ":")).encode()).decode()
request = Request(saved["url"], headers={"PAYMENT-SIGNATURE": signature})
try:
    response = build_opener(NoRedirect).open(request, timeout=30)
except HTTPError as exc:
    with exc:
        body = json.loads(exc.read())
    raise SystemExit(f"HTTP {exc.code}: {body.get('error')} — {body.get('message', '')} {body.get('hint', '')}")
with response:
    receipt = json.loads(base64.b64decode(response.headers["PAYMENT-RESPONSE"], validate=True))
    if (response.status != 200 or receipt.get("success") is not True
            or receipt.get("network") != accepted["network"]
            or receipt.get("transaction") != payment_hash):
        raise SystemExit("Invalid settlement receipt; preserve the original payment proof")
    result = json.load(response)
print(json.dumps(result))  # Save this result; another network redemption is a duplicate.
```

The snippets deliberately do not pay, retry, or fetch replacement challenges automatically. On a network failure, keep the files and follow the retry rules below. A lost response may mean the proof was already consumed. The Rust client additionally journals attempts and caches successful results.

## Failures and retries

- Invalid digit counts fail before payment. Fix them before purchasing.
- If payment is pending or the wallet times out, look up that exact payment. Never automatically start a replacement payment.
- Preserve the original challenge and proof on connection loss. Retry the same proof only for the exact original request. Never repay to recover a lost response automatically.
- Proofs are single use. `duplicate_settlement` (HTTP 409) means a previous attempt consumed the proof, potentially before a response was lost. Stop and report this; the demo has no paid-response recovery or automatic refund.
- Redemption is allowed through invoice expiry plus 60 seconds. After that, report the failure. Lightning payment precedes delivery; overpayment buys no extra credit and no protocol refund is provided.
- This skill does not make every x402 library Lightning-compatible. MDK's automatic L402 handling uses different headers; use the explicit x402 flow above.

Failed paid requests preserve the spec's stable `error` code and settlement `payment.errorReason`. The JSON body also supplies `message` and `hint` for common mistakes:

| Error | Meaning and next step |
|---|---|
| `invalid_exact_lnbtc_preimage_hash_mismatch` | The proof does not match the invoice inside `accepted`. Restore the original saved entry from the invoice you paid; do not fetch another challenge. |
| `invalid_exact_lnbtc_pay_to_mismatch` | `accepted.payTo` differs from the service's receiving node. Preserve the original entry intact and check the payload nesting. This does not mean the preimage is wrong. |
| `invalid_exact_lnbtc_invoice_payee_mismatch` | The invoice signer differs from `accepted.payTo`; fields from different challenges may have been mixed. Restore the original entry. |
| `invalid_exact_lnbtc_request_mismatch` / `invalid_exact_lnbtc_invoice_request_mismatch` | The URL or request binding differs. Use the exact original URL and query string. |

Specification: https://github.com/x402-foundation/x402/blob/main/specs/schemes/exact/scheme_exact_lnbtc.md
