//! x402 v2 exact/lnbtc/bolt11/upfront, HTTP profile only.
use base64::{Engine, engine::general_purpose::STANDARD};
use bitcoin::{hashes::Hash, secp256k1::PublicKey};
use lightning_invoice::{
    Bolt11Invoice, Bolt11InvoiceDescriptionRef, Currency, RawTaggedField, SignedRawBolt11Invoice,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

pub const NETWORK: &str = "lnbtc:000000000019d6689c085ae165831e93";
pub const SKEW: u64 = 60;
pub type Check<T> = Result<T, &'static str>;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs()
}
pub fn hash(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
pub fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn encode(v: &Value) -> String {
    STANDARD.encode(serde_json::to_vec(v).expect("JSON value"))
}

pub fn decode(s: &str) -> Check<Value> {
    if s.len() > 24_000 {
        return Err("invalid_payment_payload");
    }
    let bytes = STANDARD.decode(s).map_err(|_| "invalid_payment_payload")?;
    strict_json(&bytes).map_err(|_| "invalid_payment_payload")
}

// Reject duplicate object keys instead of letting last-key-wins change payment terms.
pub fn strict_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    struct Unique(Value);
    impl<'de> serde::Deserialize<'de> for Unique {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Unique;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    write!(f, "JSON without duplicate keys")
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique, E> {
                    serde_json::Number::from_f64(v)
                        .map(|n| Unique(Value::Number(n)))
                        .ok_or_else(|| E::custom("non-finite number"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Unique, E> {
                    Ok(Unique(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut a: A,
                ) -> Result<Unique, A::Error> {
                    let mut out = vec![];
                    while let Some(Unique(v)) = a.next_element()? {
                        out.push(v);
                    }
                    Ok(Unique(out.into()))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut a: A,
                ) -> Result<Unique, A::Error> {
                    let mut out = serde_json::Map::new();
                    while let Some((k, Unique(v))) = a.next_entry::<String, Unique>()? {
                        if out.insert(k, v).is_some() {
                            return Err(serde::de::Error::custom("duplicate JSON key"));
                        }
                    }
                    Ok(Unique(Value::Object(out)))
                }
            }
            d.deserialize_any(Visitor)
        }
    }
    serde_json::from_slice::<Unique>(bytes).map(|v| v.0)
}

pub fn request_hash(url: &str) -> String {
    let binding = json!({"domain":"x402:exact:lnbtc:bolt11:http:1", "method":"GET", "url":url, "bodyHash":hash([]), "headers":[]});
    hash(serde_jcs::to_vec(&binding).expect("string-only binding"))
}

pub fn requirements(
    url: &str,
    pay_to: &str,
    amount_msat: u64,
    expiry: u32,
    invoice: &str,
) -> Value {
    json!({"scheme":"exact","network":NETWORK,"asset":"BTC","amount":amount_msat.to_string(),"payTo":pay_to,"maxTimeoutSeconds":expiry,
        "extra":{"assetTransferMethod":"bolt11","paymentFlow":"upfront","requestBindingProfile":"http:1","requestBindingParams":{"headers":[]},"requestHash":request_hash(url),"invoice":invoice}})
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn amount(v: &Value) -> Check<u64> {
    let s = text(v, "amount");
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid_exact_lnbtc_amount");
    }
    s.parse::<u64>()
        .ok()
        .filter(|x| *x > 0)
        .ok_or("invalid_exact_lnbtc_amount")
}
fn binding(v: &Value) -> Check<()> {
    let e = &v["extra"];
    if !is_hex(text(e, "requestHash"), 64)
        || e["requestBindingProfile"] != "http:1"
        || e["requestBindingParams"] != json!({"headers":[]})
    {
        return Err("invalid_exact_lnbtc_request_binding");
    }
    Ok(())
}
pub fn validate_terms(v: &Value) -> Check<()> {
    if v["scheme"] != "exact" {
        return Err("unsupported_scheme");
    }
    if v["network"] != NETWORK {
        return Err("unsupported_network");
    }
    if v["asset"] != "BTC" {
        return Err("invalid_exact_lnbtc_asset");
    }
    amount(v)?;
    let pk = text(v, "payTo");
    if !is_hex(pk, 66) || PublicKey::from_str(pk).is_err() {
        return Err("invalid_exact_lnbtc_pay_to_malformed");
    }
    if v["maxTimeoutSeconds"].as_u64().filter(|x| *x > 0).is_none() {
        return Err("invalid_exact_lnbtc_max_timeout");
    }
    let e = &v["extra"];
    if e.get("assetTransferMethod").is_some_and(|m| m != "bolt11") {
        return Err("invalid_exact_lnbtc_asset_transfer_method");
    }
    if e["paymentFlow"] != "upfront" {
        return Err("invalid_exact_lnbtc_payment_flow");
    }
    binding(v)?;
    if text(e, "invoice").is_empty() {
        return Err("invalid_exact_lnbtc_invoice_missing");
    }
    Ok(())
}

pub struct InvoiceProof {
    pub payment_hash: String,
    pub invoice_end: u64,
}

pub fn validate_invoice(v: &Value, time: u64, paid: bool) -> Check<InvoiceProof> {
    validate_terms(v)?;
    let s = text(&v["extra"], "invoice");
    let raw = SignedRawBolt11Invoice::from_str(s)
        .map_err(|_| "invalid_exact_lnbtc_invoice_decode_failed")?;
    let mut seen = std::collections::BTreeSet::new();
    for field in &raw.raw_invoice().data.tagged_fields {
        let tag = match field {
            RawTaggedField::KnownSemantics(f) => f.tag().to_u8(),
            RawTaggedField::UnknownSemantics(v) => {
                let tag = v
                    .first()
                    .ok_or("invalid_exact_lnbtc_invoice_decode_failed")?
                    .to_u8();
                // BOLT11 parsers may preserve malformed known tags as unknown.
                // Never allow a malformed field to evade cardinality checks.
                if [1, 13, 19, 23, 6, 24, 9, 3, 16, 27, 5].contains(&tag) {
                    return Err("invalid_exact_lnbtc_invoice_decode_failed");
                }
                continue;
            }
        };
        if tag != 3 && tag != 9 && !seen.insert(tag) {
            return Err("invalid_exact_lnbtc_invoice_decode_failed");
        }
    }
    if !seen.contains(&23) || seen.contains(&13) {
        return Err("invalid_exact_lnbtc_invoice_description");
    }
    let invoice =
        Bolt11Invoice::from_signed(raw).map_err(|_| "invalid_exact_lnbtc_invoice_decode_failed")?;
    // Bolt11Invoice's semantic parser validates field cardinality, signature,
    // checksum, known feature bits, and integral millisatoshi precision.
    let expected = hex::decode(text(&v["extra"], "requestHash"))
        .map_err(|_| "invalid_exact_lnbtc_request_binding")?;
    match invoice.description() {
        Bolt11InvoiceDescriptionRef::Hash(h) if h.0.as_byte_array().as_slice() == expected => {}
        Bolt11InvoiceDescriptionRef::Hash(_) => {
            return Err("invalid_exact_lnbtc_invoice_request_mismatch");
        }
        _ => return Err("invalid_exact_lnbtc_invoice_description"),
    }
    if invoice.recover_payee_pub_key().to_string() != text(v, "payTo")
        || invoice
            .payee_pub_key()
            .is_some_and(|p| p.to_string() != text(v, "payTo"))
    {
        return Err("invalid_exact_lnbtc_invoice_payee_mismatch");
    }
    if invoice.currency() != Currency::Bitcoin {
        return Err("invalid_exact_lnbtc_invoice_currency_mismatch");
    }
    if invoice.amount_milli_satoshis() != Some(amount(v)?) {
        return Err("invalid_exact_lnbtc_invoice_amount_mismatch");
    }
    let expiry = invoice.expiry_time().as_secs();
    if Some(expiry) != v["maxTimeoutSeconds"].as_u64() {
        return Err("invalid_exact_lnbtc_invoice_expiry_mismatch");
    }
    let created = invoice.duration_since_epoch().as_secs();
    if created > time.saturating_add(SKEW) {
        return Err("invalid_exact_lnbtc_invoice_created_in_future");
    }
    let invoice_end = created
        .checked_add(expiry)
        .ok_or("invalid_exact_lnbtc_invoice_expired")?;
    if if paid {
        time > invoice_end.saturating_add(SKEW)
    } else {
        time >= invoice_end
    } {
        return Err("invalid_exact_lnbtc_invoice_expired");
    }
    Ok(InvoiceProof {
        payment_hash: invoice.payment_hash().to_string(),
        invoice_end,
    })
}

pub fn validate_payment(payload: &Value, expected: &Value, time: u64) -> Check<InvoiceProof> {
    if payload["x402Version"] != 2 {
        return Err("invalid_x402_version");
    }
    let accepted = &payload["accepted"];
    for (key, error) in [
        ("scheme", "unsupported_scheme"),
        ("network", "network_mismatch"),
        ("asset", "invalid_exact_lnbtc_asset"),
        ("amount", "invalid_exact_lnbtc_amount_mismatch"),
        ("payTo", "invalid_exact_lnbtc_pay_to_mismatch"),
        (
            "maxTimeoutSeconds",
            "invalid_exact_lnbtc_max_timeout_mismatch",
        ),
    ] {
        if accepted[key] != expected[key] {
            return Err(error);
        }
    }
    validate_terms(accepted)?;
    validate_terms(expected)?;
    for key in [
        "requestHash",
        "requestBindingProfile",
        "requestBindingParams",
    ] {
        if accepted["extra"][key] != expected["extra"][key] {
            return Err("invalid_exact_lnbtc_request_mismatch");
        }
    }
    for (key, value) in expected["extra"]
        .as_object()
        .ok_or("invalid_exact_lnbtc_extra_mismatch")?
    {
        if key != "invoice" && key != "assetTransferMethod" && accepted["extra"][key] != *value {
            return Err("invalid_exact_lnbtc_extra_mismatch");
        }
    }
    let proof = validate_invoice(accepted, time, true)?;
    let pre = payload["payload"]
        .get("preimage")
        .and_then(Value::as_str)
        .ok_or("invalid_exact_lnbtc_preimage_missing")?;
    if pre.len() != 64 {
        return Err("invalid_exact_lnbtc_preimage_length");
    }
    if !is_hex(pre, 64) {
        return Err("invalid_exact_lnbtc_preimage_malformed");
    }
    if hash(hex::decode(pre).map_err(|_| "invalid_exact_lnbtc_preimage_malformed")?)
        != proof.payment_hash
    {
        return Err("invalid_exact_lnbtc_preimage_hash_mismatch");
    }
    Ok(proof)
}

pub fn settlement(hash: &str) -> Value {
    json!({"success":true,"transaction":hash,"network":NETWORK})
}
