use bitcoin::{
    hashes::{Hash, sha256},
    secp256k1::{Secp256k1, SecretKey},
};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentHash, PaymentSecret};
use lightning_pi::protocol as p;
use serde_json::{Value, json};
use std::time::Duration;

fn fixture() -> (Value, Value) {
    let mut r: Value = serde_json::from_str(include_str!("fixtures/requirements.json")).unwrap();
    let mut payload: Value = serde_json::from_str(include_str!("fixtures/payload.json")).unwrap();
    // The published invoice omits feature bits and is rejected by LDK. Keep
    // that original as a regression fixture; use its inputs with valid features
    // for settlement tests. InvoiceBuilder adds payment_secret feature bits.
    let mut key = [0u8; 32];
    key[31] = 1;
    let key = SecretKey::from_slice(&key).unwrap();
    let pre = hex::decode(payload["payload"]["preimage"].as_str().unwrap()).unwrap();
    let invoice = InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(25000)
        .description_hash(
            sha256::Hash::from_str(r["extra"]["requestHash"].as_str().unwrap()).unwrap(),
        )
        .payment_hash(PaymentHash(sha256::Hash::hash(&pre).to_byte_array()))
        .payment_secret(PaymentSecret([0x11; 32]))
        .duration_since_epoch(Duration::from_secs(1700000000))
        .expiry_time(Duration::from_secs(300))
        .min_final_cltv_expiry_delta(18)
        .build_signed(|m| Secp256k1::new().sign_ecdsa_recoverable(m, &key))
        .unwrap();
    r["extra"]["invoice"] = json!(invoice.to_string());
    payload["accepted"] = r.clone();
    (r, payload)
}
#[test]
fn published_binding_vectors_and_equivalent_ldk_invoice() {
    let (r, payload) = fixture();
    assert_eq!(
        p::request_hash("https://api.example.com/article/A"),
        "0d6623f775e025501fa7f0a30b54da25aad62b6ccfe35c85da38016711e6c018"
    );
    assert_eq!(
        p::request_hash("https://api.example.com/article/B"),
        "4a99860f75eed1ea8178a5db488e044173bc570c8a6210f2c8590cdf8622d509"
    );
    let proof = p::validate_payment(&payload, &r, 1700000000).unwrap();
    assert_eq!(
        proof.payment_hash,
        "a923c2c0e4fe77061ff1cb882171f6fdf926719bb7f5ffe2e05458438c52825e"
    );
    assert!(p::validate_payment(&payload, &r, 1700000360).is_ok());
    assert_eq!(
        p::validate_payment(&payload, &r, 1700000361).err(),
        Some("invalid_exact_lnbtc_invoice_expired")
    );
    assert!(p::validate_payment(&payload, &r, 1699999940).is_ok());
    assert_eq!(
        p::validate_payment(&payload, &r, 1699999939).err(),
        Some("invalid_exact_lnbtc_invoice_created_in_future")
    );
    let mut other = r.clone();
    other["extra"]["invoice"] = json!("a newly issued challenge is dynamic");
    assert!(p::validate_payment(&payload, &other, 1700000000).is_ok());
}
#[test]
fn published_invoice_has_missing_ldk_features() {
    let r: Value = serde_json::from_str(include_str!("fixtures/requirements.json")).unwrap();
    let raw = lightning_invoice::SignedRawBolt11Invoice::from_str(
        r["extra"]["invoice"].as_str().unwrap(),
    )
    .unwrap();
    assert!(raw.check_signature());
    assert!(raw.raw_invoice().features().is_none());
    assert_eq!(
        lightning_invoice::Bolt11Invoice::from_signed(raw).err(),
        Some(lightning_invoice::Bolt11SemanticError::InvalidFeatures)
    );
    assert_eq!(
        p::validate_invoice(&r, 1700000000, false).err(),
        Some("invalid_exact_lnbtc_invoice_decode_failed")
    );
}
#[test]
fn rejects_cross_request_and_bad_proofs() {
    let (r, payload) = fixture();
    let mut other = r.clone();
    other["extra"]["requestHash"] = json!(p::request_hash("https://api.example.com/article/B"));
    assert_eq!(
        p::validate_payment(&payload, &other, 1700000000).err(),
        Some("invalid_exact_lnbtc_request_mismatch")
    );
    let mut forged = payload.clone();
    forged["accepted"]["extra"]["requestHash"] = other["extra"]["requestHash"].clone();
    assert_eq!(
        p::validate_payment(&forged, &other, 1700000000).err(),
        Some("invalid_exact_lnbtc_invoice_request_mismatch")
    );
    for (pre, reason) in [
        (
            "00".repeat(32),
            "invalid_exact_lnbtc_preimage_hash_mismatch",
        ),
        ("AB".repeat(32), "invalid_exact_lnbtc_preimage_malformed"),
        ("00".repeat(31), "invalid_exact_lnbtc_preimage_length"),
    ] {
        let mut bad = payload.clone();
        bad["payload"]["preimage"] = json!(pre);
        assert_eq!(
            p::validate_payment(&bad, &r, 1700000000).err(),
            Some(reason)
        );
    }
    for key in [
        "requestHash",
        "requestBindingProfile",
        "requestBindingParams",
    ] {
        let mut bad = payload.clone();
        bad["accepted"]["extra"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert_eq!(
            p::validate_payment(&bad, &r, 1700000000).err(),
            Some("invalid_exact_lnbtc_request_binding")
        );
    }
    for (key, value) in [
        ("paymentFlow", json!("authorization")),
        ("assetTransferMethod", json!("default")),
        ("requestBindingProfile", json!("mcp:1")),
        ("requestBindingParams", json!({"headers":[],"extra":true})),
    ] {
        let mut bad = payload.clone();
        bad["accepted"]["extra"][key] = value;
        assert!(p::validate_payment(&bad, &r, 1700000000).is_err());
    }
}
#[test]
fn rejects_price_payee_and_network_changes() {
    let (r, payload) = fixture();
    for (key, value) in [
        ("amount", json!("1")),
        ("asset", json!("ETH")),
        ("network", json!("bip122:000000000019d6689c085ae165831e93")),
        ("payTo", json!("bad")),
        ("maxTimeoutSeconds", json!(301)),
    ] {
        let mut bad = payload.clone();
        bad["accepted"][key] = value;
        assert!(p::validate_payment(&bad, &r, 1700000000).is_err());
    }
    for v in ["0", "-1", "1.0", "1e3", "+1", ""] {
        let mut bad = r.clone();
        bad["amount"] = json!(v);
        assert_eq!(
            p::validate_terms(&bad).err(),
            Some("invalid_exact_lnbtc_amount")
        );
    }
}
#[test]
fn duplicate_json_keys_rejected() {
    assert!(p::strict_json(br#"{"payload":{"preimage":"a","preimage":"b"}}"#).is_err());
    assert!(p::strict_json(br#"{"payload":{"preimage":"a"}} trailing"#).is_err());
}
#[test]
fn signed_wrong_amount_and_description_rejected() {
    let (mut r, _) = fixture();
    let key = SecretKey::from_slice(&[
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 1,
    ])
    .unwrap();
    let builder = InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(26000)
        .description_hash(
            sha256::Hash::from_str(r["extra"]["requestHash"].as_str().unwrap()).unwrap(),
        )
        .payment_hash(PaymentHash([1; 32]))
        .payment_secret(PaymentSecret([2; 32]))
        .duration_since_epoch(Duration::from_secs(1700000000))
        .expiry_time(Duration::from_secs(300))
        .min_final_cltv_expiry_delta(18);
    let invoice = builder
        .build_signed(|m| Secp256k1::new().sign_ecdsa_recoverable(m, &key))
        .unwrap();
    r["extra"]["invoice"] = json!(invoice.to_string());
    assert_eq!(
        p::validate_invoice(&r, 1700000000, false).err(),
        Some("invalid_exact_lnbtc_invoice_amount_mismatch")
    );
    let invoice = InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(25000)
        .description("plain".into())
        .payment_hash(PaymentHash([1; 32]))
        .payment_secret(PaymentSecret([2; 32]))
        .duration_since_epoch(Duration::from_secs(1700000000))
        .expiry_time(Duration::from_secs(300))
        .min_final_cltv_expiry_delta(18)
        .build_signed(|m| Secp256k1::new().sign_ecdsa_recoverable(m, &key))
        .unwrap();
    r["extra"]["invoice"] = json!(invoice.to_string());
    assert_eq!(
        p::validate_invoice(&r, 1700000000, false).err(),
        Some("invalid_exact_lnbtc_invoice_description")
    );
}
use std::str::FromStr;
