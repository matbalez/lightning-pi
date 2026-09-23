use crate::{pi, protocol as p, store::ReplayStore, wallet::create_bound_invoice};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

#[async_trait]
pub trait Receiver: Send + Sync {
    fn pay_to(&self) -> &str;
    async fn invoice(&self, amount: u64, hash: [u8; 32], expiry: u32) -> Result<String>;
}
pub struct LexeReceiver {
    pub wallet: lexe::wallet::LexeWallet,
    pub pay_to: String,
}
#[async_trait]
impl Receiver for LexeReceiver {
    fn pay_to(&self) -> &str {
        &self.pay_to
    }
    async fn invoice(&self, amount: u64, hash: [u8; 32], expiry: u32) -> Result<String> {
        create_bound_invoice(&self.wallet, amount, hash, expiry).await
    }
}

#[derive(Clone)]
pub struct Config {
    pub origin: String,
    pub authority: String,
    pub amount: u64,
    pub max_digits: u32,
    pub expiry: u32,
}
impl Config {
    pub fn new(origin: String, amount: u64, max_digits: u32) -> Result<Self> {
        let u = url::Url::parse(&origin)?;
        ensure!(
            u.scheme() == "https"
                || (u.scheme() == "http"
                    && matches!(u.host_str(), Some("localhost" | "127.0.0.1"))),
            "Use HTTPS except on loopback"
        );
        ensure!(
            u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none()
                && u.path() == "/",
            "PUBLIC_ORIGIN must contain only scheme and authority"
        );
        let canonical = u.origin().ascii_serialization();
        ensure!(
            origin.trim_end_matches('/') == canonical,
            "PUBLIC_ORIGIN must be canonical ASCII"
        );
        ensure!(
            amount > 0 && amount <= 100_000_000_000 && amount.is_multiple_of(1000),
            "Price must be a positive whole bitcoin base-unit amount"
        );
        ensure!(max_digits <= 10_000, "MAX_DIGITS must be <= 10000");
        let authority = canonical.split_once("://").unwrap().1.to_owned();
        Ok(Self {
            origin: canonical,
            authority,
            amount,
            max_digits,
            expiry: 300,
        })
    }
}
struct AppState {
    config: Config,
    receiver: Arc<dyn Receiver>,
    settle_url: String,
    http: reqwest::Client,
    invoice_slots: Semaphore,
    work_slots: Semaphore,
    rate: Mutex<(Instant, u32)>,
}
pub fn app(config: Config, receiver: Arc<dyn Receiver>, settle_url: String) -> Router {
    let state = Arc::new(AppState {
        config,
        receiver,
        settle_url,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .no_proxy()
            .build()
            .unwrap(),
        invoice_slots: Semaphore::new(4),
        work_slots: Semaphore::new(8),
        rate: Mutex::new((Instant::now(), 0)),
    });
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(|| async { Json(json!({"ok":true})) }))
        .route("/digits-of-pi", get(digits))
        .route(
            "/SKILL.md",
            get(|| async {
                (
                    [("content-type", "text/markdown; charset=utf-8")],
                    include_str!("../skills/x402-lightning/SKILL.md"),
                )
            }),
        )
        .layer(DefaultBodyLimit::max(0))
        .with_state(state)
}
async fn index(State(s): State<Arc<AppState>>) -> Json<Value> {
    Json(
        json!({"service":"Lightning pi","endpoint":"/digits-of-pi?digits=100","priceMsat":s.config.amount.to_string(),"price":format!("₿{}",s.config.amount/1000),"maxDigits":s.config.max_digits,"rounding":"nearest decimal, returned as a string","network":p::NETWORK,"x402Version":2,"skill":format!("{}/SKILL.md",s.config.origin),"source":"https://github.com/matbalez/lightning-pi"}),
    )
}
fn response(status: StatusCode, value: Value) -> Response {
    let mut r = (status, Json(value)).into_response();
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r
}
fn error(status: StatusCode, reason: &str) -> Response {
    response(status, error_body(reason))
}
fn error_body(reason: &str) -> Value {
    // Preserve the spec's stable codes. Give agents actionable context without
    // echoing invoices, preimages, or other submitted payment data.
    let help = match reason {
        "invalid_exact_lnbtc_preimage_hash_mismatch" => Some((
            "The preimage does not match the invoice in PAYMENT-SIGNATURE.accepted.extra.invoice.",
            "Reuse the saved accepts entry from the challenge whose invoice you paid. A fresh unpaid GET creates a different invoice; do not combine its challenge with the original payment proof or pay again automatically.",
        )),
        "invalid_exact_lnbtc_pay_to_mismatch" => Some((
            "PAYMENT-SIGNATURE.accepted.payTo differs from this service's configured receiving node.",
            "Copy the entire original accepts entry into accepted, preserving payTo and extra.invoice. Do not use the whole PAYMENT-REQUIRED object or an accepts array. This code identifies a recipient-field mismatch, not a preimage mismatch.",
        )),
        "invalid_exact_lnbtc_invoice_payee_mismatch" => Some((
            "The invoice signing key does not match accepted.payTo.",
            "Restore the original accepted entry intact. Do not combine an invoice with fields copied from another challenge.",
        )),
        "invalid_exact_lnbtc_request_mismatch" | "invalid_exact_lnbtc_invoice_request_mismatch" => {
            Some((
                "The submitted challenge or invoice is bound to a different request.",
                "Retry the exact saved URL, including its query string, using the original accepted entry and payment proof.",
            ))
        }
        "invalid_exact_lnbtc_preimage_missing" => Some((
            "PAYMENT-SIGNATURE.payload.preimage is missing.",
            "Use {x402Version: 2, accepted: ORIGINAL_ACCEPTS_ENTRY, payload: {preimage: HEX}}. The preimage comes from your completed wallet payment, not the invoice or payment hash.",
        )),
        "invalid_exact_lnbtc_invoice_expired" => Some((
            "The original invoice's redemption window has expired.",
            "Keep the original challenge and payment record. Fetching a new challenge cannot renew an already paid invoice; do not pay again automatically.",
        )),
        "duplicate_settlement" => Some((
            "This invoice's payment proof has already been consumed.",
            "Use your saved successful result if available. If the response was lost, report the failure; this service has no paid-response recovery. Do not pay again automatically.",
        )),
        "invalid_payment_payload" => Some((
            "PAYMENT-SIGNATURE must be base64-encoded JSON with no duplicate keys.",
            "Use {x402Version: 2, accepted: ORIGINAL_ACCEPTS_ENTRY, payload: {preimage: HEX}}. Send standard base64, not raw JSON or an L402 Authorization header.",
        )),
        _ => None,
    };
    let mut body = json!({"error":reason});
    if let Some((message, hint)) = help {
        body["message"] = json!(message);
        body["hint"] = json!(hint);
    }
    body
}
fn rate_error() -> Response {
    let mut r = error(StatusCode::TOO_MANY_REQUESTS, "busy_retry_later");
    r.headers_mut()
        .insert("retry-after", HeaderValue::from_static("60"));
    r
}
fn parse_digits(uri: &Uri, max: u32) -> Option<u32> {
    let q = uri.query()?.strip_prefix("digits=")?;
    if q.is_empty()
        || q.len() > 5
        || !q.bytes().all(|c| c.is_ascii_digit())
        || (q.len() > 1 && q.starts_with('0'))
    {
        return None;
    }
    q.parse().ok().filter(|d| *d <= max)
}
async fn digits(
    State(s): State<Arc<AppState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if method != Method::GET {
        return error(StatusCode::METHOD_NOT_ALLOWED, "use_GET");
    }
    if !body.is_empty() || headers.contains_key("content-encoding") {
        return error(
            StatusCode::BAD_REQUEST,
            "GET_body_and_content_encoding_not_supported",
        );
    }
    if headers.get_all("host").iter().count() != 1
        || headers.get("host").and_then(|h| h.to_str().ok()) != Some(s.config.authority.as_str())
        || uri.scheme().is_some()
    {
        return error(StatusCode::BAD_REQUEST, "invalid_public_origin");
    }
    let Some(d) = parse_digits(&uri, s.config.max_digits) else {
        return error(
            StatusCode::BAD_REQUEST,
            "digits_must_be_one_integer_between_0_and_maxDigits",
        );
    };
    let url = format!(
        "{}{}",
        s.config.origin,
        uri.path_and_query().unwrap().as_str()
    );
    if headers.get_all("payment-signature").iter().count() > 1 {
        return error(
            StatusCode::BAD_REQUEST,
            "duplicate_payment_signature_header",
        );
    }
    if let Some(signature) = headers.get("payment-signature") {
        let _permit = match s.work_slots.try_acquire() {
            Ok(p) => p,
            Err(_) => return rate_error(),
        };
        let payload = match signature.to_str().ok().and_then(|v| p::decode(v).ok()) {
            Some(v) => v,
            None => return error(StatusCode::BAD_REQUEST, "invalid_payment_payload"),
        };
        let invoice = payload["accepted"]["extra"]["invoice"]
            .as_str()
            .unwrap_or("");
        let expected = p::requirements(
            &url,
            s.receiver.pay_to(),
            s.config.amount,
            s.config.expiry,
            invoice,
        );
        // All authoritative inputs come from this request/configuration. Only
        // the dynamic invoice comes from the original challenge being redeemed.
        let result = s
            .http
            .post(&s.settle_url)
            .json(&json!({"x402Version":2,"paymentPayload":payload,"paymentRequirements":expected}))
            .send()
            .await;
        let settlement = match result {
            Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
            _ => None,
        };
        let Some(settlement) = settlement else {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "settlement_unavailable_preserve_payment_proof",
            );
        };
        if settlement["success"] != true {
            // Never silently issue a replacement invoice after a paid failure.
            let reason = settlement["errorReason"]
                .as_str()
                .unwrap_or("settlement_failed");
            let status = if reason == "duplicate_settlement" {
                StatusCode::CONFLICT
            } else {
                StatusCode::PAYMENT_REQUIRED
            };
            let mut body = error_body(reason);
            body["payment"] = settlement.clone();
            let mut r = response(status, body);
            r.headers_mut().insert(
                "payment-response",
                HeaderValue::from_str(&p::encode(&settlement)).unwrap(),
            );
            return r;
        }
        let value = match tokio::task::spawn_blocking(move || pi::rounded(d)).await {
            Ok(v) => v,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "calculation_failed_after_settlement",
                );
            }
        };
        let mut r = response(
            StatusCode::OK,
            json!({"digits":d,"pi":value,"rounding":"nearest"}),
        );
        r.headers_mut().insert(
            "payment-response",
            HeaderValue::from_str(&p::encode(&settlement)).unwrap(),
        );
        return r;
    }
    let _slot = match s.invoice_slots.try_acquire() {
        Ok(p) => p,
        Err(_) => return rate_error(),
    };
    {
        let mut rate = s.rate.lock().unwrap();
        if rate.0.elapsed() >= Duration::from_secs(60) {
            *rate = (Instant::now(), 0);
        }
        if rate.1 >= 120 {
            return rate_error();
        }
        rate.1 += 1;
    }
    let hash: [u8; 32] = hex::decode(p::request_hash(&url))
        .unwrap()
        .try_into()
        .unwrap();
    let invoice = match tokio::time::timeout(
        Duration::from_secs(25),
        s.receiver.invoice(s.config.amount, hash, s.config.expiry),
    )
    .await
    {
        Ok(Ok(i)) => i,
        _ => return error(StatusCode::SERVICE_UNAVAILABLE, "invoice_unavailable"),
    };
    let req = p::requirements(
        &url,
        s.receiver.pay_to(),
        s.config.amount,
        s.config.expiry,
        &invoice,
    );
    if let Err(reason) = p::validate_invoice(&req, p::now(), false) {
        tracing::error!(reason, "Receiver returned invalid invoice");
        return error(StatusCode::SERVICE_UNAVAILABLE, "receiver_invoice_invalid");
    }
    let challenge = json!({"x402Version":2,"resource":{"url":url,"description":"Pi rounded to the requested decimal places","mimeType":"application/json","serviceName":"Lightning pi"},"accepts":[req]});
    let mut r = response(StatusCode::PAYMENT_REQUIRED, challenge.clone());
    r.headers_mut().insert(
        "payment-required",
        HeaderValue::from_str(&p::encode(&challenge)).unwrap(),
    );
    r
}

pub fn facilitator(store: Arc<ReplayStore>) -> Router {
    Router::new()
        .route("/settle", post(settle))
        .layer(DefaultBodyLimit::max(32_000))
        .with_state(store)
}
async fn settle(State(store): State<Arc<ReplayStore>>, body: Bytes) -> Response {
    let request = match p::strict_json(&body) {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_payment_payload"),
    };
    let time = p::now();
    let checked = if request["x402Version"] != 2 {
        Err("invalid_x402_version")
    } else {
        p::validate_payment(
            &request["paymentPayload"],
            &request["paymentRequirements"],
            time,
        )
    };
    match checked {
        Ok(proof) => match store.consume(&proof.payment_hash, proof.invoice_end, time) {
            Ok(true) => response(StatusCode::OK, p::settlement(&proof.payment_hash)),
            Ok(false) => response(
                StatusCode::OK,
                json!({"success":false,"errorReason":"duplicate_settlement","transaction":"","network":p::NETWORK}),
            ),
            Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "replay_store_unavailable"),
        },
        Err(reason) => response(
            StatusCode::OK,
            json!({"success":false,"errorReason":reason,"transaction":"","network":p::NETWORK}),
        ),
    }
}
