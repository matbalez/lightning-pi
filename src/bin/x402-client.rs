//! Deterministic payer: validates before paying, journals before side effects.
use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use lexe::{
    types::{
        bitcoin::Invoice,
        payment::{Payment, PaymentCreatedIndex, PaymentStatus},
    },
    wallet::LexeWallet,
};
use lexe_api_core::{
    def::UserNodeRunApi,
    models::command::{PayInvoicePreflightRequest, PayInvoiceRequest, PaymentIdStruct},
    types::payments::PaymentKind,
};
use lightning_pi::{protocol as p, wallet::load_wallet};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "x402-client",
    about = "Pay an x402 Lightning endpoint with validated, recoverable state"
)]
struct Cli {
    /// Private journal file; never commit or upload it.
    #[arg(long)]
    state: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Fetch and validate a challenge without spending.
    Prepare {
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 100_000)]
        max_amount_msat: u64,
    },
    /// Prepare, pay using Lexe, and redeem exactly once.
    Buy {
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 100_000)]
        max_amount_msat: u64,
        #[arg(long, default_value_t = 10_000)]
        max_fee_msat: u64,
    },
    /// Pay a prepared challenge using the configured Lexe wallet.
    PayLexe {
        #[arg(long, default_value_t = 10_000)]
        max_fee_msat: u64,
    },
    /// Query the original payment after interruption; never initiates a payment.
    RecoverLexe,
    /// Attach a completed payment from another wallet's adapter.
    AttachResult {
        #[arg(long)]
        payment_result: PathBuf,
    },
    /// Import a completed payment from MDK agent-wallet payments JSON.
    AttachMdk {
        #[arg(long)]
        payments_file: PathBuf,
    },
    /// Redeem the saved proof without paying again.
    Redeem,
}
fn check<T>(v: p::Check<T>) -> Result<T> {
    v.map_err(|e| anyhow::anyhow!(e))
}
fn save(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(value)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}
fn load(path: &Path) -> Result<Value> {
    Ok(p::strict_json(&std::fs::read(path)?)?)
}
fn validate_state(s: &Value, paid: bool) -> Result<p::InvoiceProof> {
    let url = s["url"].as_str().context("Missing URL")?;
    ensure!(
        s["accepted"]["extra"]["requestHash"] == p::request_hash(url),
        "State is not bound to the intended URL"
    );
    check(p::validate_invoice(&s["accepted"], p::now(), paid))
}
async fn prepare(http: &reqwest::Client, path: &Path, url: &str, max: u64) -> Result<()> {
    ensure!(
        !path.exists(),
        "State already exists; resume it instead of buying again"
    );
    let u = url::Url::parse(url)?;
    ensure!(
        u.as_str() == url
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none(),
        "Use an exact ASCII URL without credentials or fragment"
    );
    ensure!(
        u.scheme() == "https"
            || (u.scheme() == "http" && matches!(u.host_str(), Some("127.0.0.1" | "localhost"))),
        "HTTPS required"
    );
    let response = http.get(url).send().await?;
    ensure!(
        response.status() == 402,
        "Expected HTTP 402, got {}",
        response.status()
    );
    let challenge = check(p::decode(
        response
            .headers()
            .get("payment-required")
            .context("Missing PAYMENT-REQUIRED")?
            .to_str()?,
    ))?;
    ensure!(
        challenge["x402Version"] == 2 && challenge["resource"]["url"] == url,
        "Challenge version or resource mismatch"
    );
    ensure!(
        challenge
            .get("extensions")
            .is_none_or(|v| v.as_object().is_some_and(|m| m.is_empty())),
        "Unsupported extensions"
    );
    let accepted = challenge["accepts"]
        .as_array()
        .context("Missing accepts")?
        .iter()
        .find(|a| a["scheme"] == "exact" && a["network"] == p::NETWORK)
        .context("No supported Lightning requirement")?
        .clone();
    let state = json!({"url":url,"accepted":accepted,"phase":"prepared"});
    let proof = validate_state(&state, false)?;
    let amount = accepted["amount"]
        .as_str()
        .context("Missing amount")?
        .parse::<u64>()?;
    ensure!(amount <= max, "Invoice exceeds the authorized amount");
    ensure!(
        proof.invoice_end > p::now() + 30,
        "Invoice expires too soon"
    );
    save(path, &state)?;
    println!(
        "{}",
        json!({"status":"prepared","amountMsat":amount.to_string(),"invoice":accepted["extra"]["invoice"],"paymentHash":proof.payment_hash})
    );
    Ok(())
}
fn attach(path: &Path, mut s: Value, result: Value) -> Result<()> {
    let invoice = s["accepted"]["extra"]["invoice"]
        .as_str()
        .context("Missing invoice")?;
    let parsed: Invoice = invoice.parse()?;
    ensure!(
        result["status"] == "paid",
        "Wallet payment is not completed; preserve state and look it up"
    );
    ensure!(
        result["invoice"] == invoice
            && result["paymentHash"] == parsed.payment_hash().to_string()
            && result["amountMsat"] == s["accepted"]["amount"],
        "Wallet result does not match the invoice, hash and face amount"
    );
    let pre = result["preimage"]
        .as_str()
        .context("Wallet did not return the preimage")?;
    ensure!(
        p::is_hex(pre, 64) && p::hash(hex::decode(pre)?) == parsed.payment_hash().to_string(),
        "Invalid payment preimage"
    );
    s["payment"] = result;
    s["phase"] = json!("paid");
    save(path, &s)?;
    println!(
        "{}",
        json!({"status":"paid","paymentHash":parsed.payment_hash().to_string()})
    );
    Ok(())
}
fn adapter_result(payment: Payment) -> Value {
    // Lexe deliberately redacts PaymentPreimage's Display implementation.
    // Its Serialize implementation emits the actual hex proof for the wire.
    json!({"status":if payment.status==PaymentStatus::Completed {"paid"} else if payment.status==PaymentStatus::Pending {"in_flight"} else {"failed"},"invoice":payment.invoice.map(|i|i.to_string()),"paymentHash":payment.hash.map(|h|h.to_string()),"amountMsat":payment.amount.map(|a|a.msat().to_string()),"feeMsat":payment.fees.msat().to_string(),"preimage":payment.preimage})
}

fn mdk_result(state: &Value, history: &Value) -> Result<Value> {
    let invoice = state["accepted"]["extra"]["invoice"]
        .as_str()
        .context("Missing invoice")?;
    let records = history["payments"]
        .as_array()
        .context("Expected MDK payments JSON")?;
    let matches: Vec<_> = records
        .iter()
        .filter(|r| {
            r["destination"] == invoice
                && r["direction"] == "outbound"
                && r["status"] == "completed"
        })
        .collect();
    ensure!(
        matches.len() == 1,
        "Need exactly one completed outbound MDK payment for this invoice; do not send it again"
    );
    let r = matches[0];
    let amount = r["amountSats"]
        .as_u64()
        .and_then(|n| n.checked_mul(1000))
        .context("Expected a whole base-unit MDK amount")?;
    Ok(
        json!({"status":"paid","invoice":r["destination"],"paymentHash":r["paymentHash"],"amountMsat":amount.to_string(),"preimage":r["preimage"]}),
    )
}
async fn pay_lexe(path: &Path, max_fee: u64) -> Result<()> {
    let mut s = load(path)?;
    ensure!(
        s["phase"] == "prepared",
        "Payment already attempted; use recover-lexe or redeem"
    );
    let proof = validate_state(&s, false)?;
    ensure!(
        proof.invoice_end > p::now() + 30,
        "Invoice expires too soon; no payment attempted"
    );
    let wallet = load_wallet()?;
    let invoice: Invoice = s["accepted"]["extra"]["invoice"]
        .as_str()
        .context("Missing invoice")?
        .parse()?;
    let preflight = wallet
        .node_client()
        .pay_invoice_preflight(PayInvoicePreflightRequest {
            invoice: invoice.clone(),
            fallback_amount: None,
            kind: PaymentKind::Invoice,
        })
        .await?;
    ensure!(
        Some(preflight.amount) == invoice.amount(),
        "Route would overpay the invoice"
    );
    ensure!(
        preflight.fees.msat() <= max_fee,
        "Routing fee exceeds limit"
    );
    // Save before sending; interrupted calls must use a read-only lookup.
    s["phase"] = json!("payment_attempted");
    save(path, &s)?;
    let id = invoice.payment_id();
    let started = wallet
        .node_client()
        .pay_invoice(PayInvoiceRequest {
            invoice,
            fallback_amount: None,
            message: None,
            personal_note: None,
            kind: PaymentKind::Invoice,
            ldk_route: Some(preflight.ldk_route),
        })
        .await
        .context(
            "Payment outcome uncertain; use recover-lexe, never replace the invoice automatically",
        )?;
    let index = PaymentCreatedIndex {
        created_at: started.created_at,
        id,
    };
    s["paymentIndex"] = json!(index.to_string());
    save(path, &s)?;
    let payment = wallet
        .wait_for_payment(index, Some(Duration::from_secs(90)))
        .await
        .context("Payment may be in flight; use recover-lexe")?;
    attach(path, s, adapter_result(payment))
}
async fn recover_lexe(path: &Path) -> Result<()> {
    let s = load(path)?;
    ensure!(
        s["phase"] == "payment_attempted",
        "Use recovery only after a payment attempt"
    );
    let wallet: LexeWallet = load_wallet()?;
    let invoice: Invoice = s["accepted"]["extra"]["invoice"]
        .as_str()
        .context("Missing invoice")?
        .parse()?;
    let found = wallet
        .node_client()
        .get_payment_by_id(PaymentIdStruct {
            id: invoice.payment_id(),
        })
        .await?
        .maybe_payment
        .context("No payment found; no new payment was sent")?;
    attach(path, s, adapter_result(Payment::from(found)))
}
async fn redeem(http: &reqwest::Client, path: &Path) -> Result<()> {
    let mut s = load(path)?;
    if s["phase"] == "complete" {
        println!("{}", s["result"]);
        return Ok(());
    }
    ensure!(
        s["phase"] == "paid" || s["phase"] == "redemption_attempted",
        "No paid proof; do not buy again automatically"
    );
    validate_state(&s, true)?;
    let payload = json!({"x402Version":2,"accepted":s["accepted"],"payload":{"preimage":s["payment"]["preimage"]}});
    let proof = check(p::validate_payment(&payload, &s["accepted"], p::now()))?;
    s["phase"] = json!("redemption_attempted");
    save(path, &s)?;
    let response = http
        .get(s["url"].as_str().context("Missing URL")?)
        .header("payment-signature", p::encode(&payload))
        .send()
        .await
        .context("Response uncertain; saved proof retained; redeem again without paying")?;
    let status = response.status();
    let receipt = response
        .headers()
        .get("payment-response")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| p::decode(h).ok());
    let bytes = response.bytes().await?;
    let body = p::strict_json(&bytes)?;
    if status != 200 {
        s["lastError"] = body;
        save(path, &s)?;
        bail!(
            "Redemption returned HTTP {status}: {}. Original proof retained; do not pay again automatically.",
            s["lastError"]
        );
    }
    let receipt = receipt.context("Missing settlement receipt; proof retained")?;
    ensure!(
        receipt["success"] == true
            && receipt["network"] == p::NETWORK
            && receipt["transaction"] == proof.payment_hash,
        "Settlement receipt mismatch"
    );
    ensure!(body["pi"].is_string(), "Missing decimal string");
    s["result"] = body;
    s["receipt"] = receipt;
    s["phase"] = json!("complete");
    save(path, &s)?;
    println!("{}", s["result"]);
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let lock = opts.open(cli.state.with_extension("lock"))?;
    lock.try_lock_exclusive()
        .context("Another client is using this state file")?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(40))
        .https_only(false)
        .build()?;
    match cli.command {
        Command::Prepare {
            url,
            max_amount_msat,
        } => prepare(&http, &cli.state, &url, max_amount_msat).await?,
        Command::Buy {
            url,
            max_amount_msat,
            max_fee_msat,
        } => {
            prepare(&http, &cli.state, &url, max_amount_msat).await?;
            pay_lexe(&cli.state, max_fee_msat).await?;
            redeem(&http, &cli.state).await?;
        }
        Command::PayLexe { max_fee_msat } => pay_lexe(&cli.state, max_fee_msat).await?,
        Command::RecoverLexe => recover_lexe(&cli.state).await?,
        Command::AttachResult { payment_result } => {
            let s = load(&cli.state)?;
            ensure!(
                s["phase"] == "prepared" || s["phase"] == "payment_attempted",
                "State already contains a payment"
            );
            attach(&cli.state, s, load(&payment_result)?)?;
        }
        Command::AttachMdk { payments_file } => {
            let s = load(&cli.state)?;
            ensure!(
                s["phase"] == "prepared" || s["phase"] == "payment_attempted",
                "State already contains a payment"
            );
            let result = mdk_result(&s, &load(&payments_file)?)?;
            attach(&cli.state, s, result)?;
        }
        Command::Redeem => redeem(&http, &cli.state).await?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lexe_adapter_exports_actual_preimage_not_redacted_display() {
        let pre = "0001020304050607080900010203040506070809000102030405060708090102";
        let hash = p::hash(hex::decode(pre).unwrap());
        let payment: Payment = serde_json::from_value(json!({
            "index":format!("0000001700000000000-ln_{hash}"),
            "rail":"invoice","kind":"invoice","direction":"outbound",
            "hash":hash,"preimage":pre,"amount":"25","fees":"1",
            "status":"completed","status_msg":"completed",
            "created_at":1700000000000u64,"updated_at":1700000001000u64
        }))
        .unwrap();
        let result = adapter_result(payment);
        assert_eq!(result["preimage"], pre);
        assert_eq!(result["amountMsat"], "25000");
        assert_eq!(result["feeMsat"], "1000");
        assert_eq!(result["paymentHash"], hash);
        assert_eq!(result["status"], "paid");
    }
}
