//! Minimal Decentralized Claims Protocol (DCP) *verification* support for
//! the DSP catalog endpoints - the "narrowed (b)" scope from
//! `compliance/benchmark-2026-08-27.md`'s follow-up work: validate an
//! incoming self-issued token/Verifiable Presentation from a caller who
//! already has real DID/Credential-Service infrastructure, without
//! implementing DCP's issuer/holder-wallet side (this project has no
//! credentials of its own to present - see
//! `compliance/dcp-test-env/README.md` for the real, running
//! `eclipse-edc/IdentityHub` + Issuer Service this was built and tested
//! against).
//!
//! This module hand-rolls JWS (ES256) signing/verification and does
//! its own `did:web` resolution over plain HTTP requests, rather than
//! pulling in a JSON-LD or full JWT-framework crate: the two operations
//! actually needed are "sign this JSON with my key" and "verify this
//! compact JWS against a JWK's raw x/y", and every message shape here
//! (self-issued tokens, `PresentationQueryMessage`, the
//! `PresentationResponseMessage`) is a small, fixed, already-known
//! structure - there's no general JSON-LD processing or arbitrary JWT
//! claim-set handling to justify a heavier dependency.
//!
//! ## The flow this implements
//!
//! 1. A caller ("holder") presents `Authorization: Bearer <T1>` on a DSP
//!    request, where T1 is a self-issued JWT (signed with the holder's
//!    own `did:web` key) containing a nested `token` claim (T2 - a
//!    presentation-access-token, itself a JWT, scoped and audience-
//!    restricted to the holder's own DID).
//! 2. This connector resolves the holder's DID document, verifies T1's
//!    signature against it, and checks `aud` matches this connector's
//!    own DID (see `own_did_document`, hosted at `GET /dsp/did.json`).
//! 3. **Proof of original possession**: DCP requires the party that
//!    received T1 (this connector) to re-package the nested T2 into a
//!    *new* self-issued token (T3), signed with *this connector's own*
//!    key, before the holder's Presentation API will honor it - a bare
//!    forward of T2 is rejected (`compliance/dcp-test-env/README.md`
//!    documents the trail of 401s that surfaced this). This connector
//!    signs T3 itself, in-process - no separate STS service, since it
//!    has no need to issue tokens for any other purpose.
//! 4. T3 is POSTed to the holder's Presentation API (discovered from the
//!    holder's DID document's `CredentialService` entry), requesting
//!    `required_scope`. The response is a signed VerifiablePresentation
//!    (a JWS) wrapping one or more Verifiable Credentials (also JWS).
//! 5. The VP's signature is checked against the holder's DID (again),
//!    and each embedded VC's signature is checked against *its own*
//!    issuer's DID (resolved separately) plus expiry. The verified
//!    credential(s)' `catalogAccess` claim becomes this caller's
//!    dataset allow-list - see `visible_datasets` in `lib.rs` for the
//!    bearer-mode equivalent this mirrors.
//!
//! What this deliberately does not do: verify credential *status*
//! (revocation lists), enforce a trusted-issuer allowlist (any issuer
//! whose DID resolves and whose signature checks out is accepted - a
//! real deployment should add one), or support any VC format other than
//! the JWT-VC (`VC1_0_JWT`) shape this was built and tested against.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::FromEncodedPoint;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

const PRESENTATION_QUERY_CONTEXT: &str = "https://w3id.org/dspace-dcp/v1.0/dcp.jsonld";
const EXPECTED_CREDENTIAL_TYPE: &str = "FederatedCatalogAccessCredential";

/// Config for `DspAuthMode::Dcp`. Keeps only plain, `Debug`/`Clone`-able
/// data (the signing key as raw scalar bytes, not a `p256` key object)
/// so it composes trivially with `DspAuthConfig`'s existing derives;
/// `signing_key()`/`verifying_key()` reconstruct the actual key types on
/// demand, which is cheap.
#[derive(Debug, Clone)]
pub struct DcpConfig {
    /// This connector's own `did:web` identifier, e.g.
    /// `did:web:localhost%3A18080:dsp`. Advertised as the `aud` incoming
    /// self-issued tokens must target, and as the `iss`/`sub` of the
    /// re-packaged token this connector sends onward.
    pub own_did: String,
    /// Full `<own_did>#<fragment>` form used as the JWS `kid` header on
    /// tokens this connector signs, and as the matching
    /// `verificationMethod.id` in its own hosted DID document.
    pub own_key_id: String,
    pub signing_key_bytes: [u8; 32],
    /// Uncompressed public key point, as (x, y) big-endian byte arrays -
    /// kept alongside the private scalar so `own_did_document()` doesn't
    /// need to re-derive it on every request.
    pub public_key_xy: ([u8; 32], [u8; 32]),
    /// Whether to resolve `did:web` DIDs over plain HTTP instead of
    /// HTTPS. `did:web` resolution defaults to HTTPS per spec; this
    /// exists only for `compliance/dcp-test-env`'s local, unencrypted
    /// IdentityHub/Issuer Service instances (mirrors
    /// `EDC_IAM_DID_WEB_USE_HTTPS=false`, the same setting that
    /// environment's own EDC connectors need).
    pub insecure_http: bool,
    /// The DCP scope string requested from the holder's Presentation
    /// API, e.g. `org.eclipse.dspace.dcp.vc.type:FederatedCatalogAccessCredential:read`.
    pub required_scope: String,
}

impl DcpConfig {
    pub fn generate(own_did: String, insecure_http: bool, required_scope: String) -> Self {
        let signing_key = SigningKey::random(&mut rand::rngs::OsRng);
        let verifying_key = VerifyingKey::from(&signing_key);
        let point = verifying_key.to_encoded_point(false);
        let x: [u8; 32] = point.x().expect("uncompressed point has x").as_slice().try_into().expect("32 bytes");
        let y: [u8; 32] = point.y().expect("uncompressed point has y").as_slice().try_into().expect("32 bytes");
        Self {
            own_key_id: format!("{own_did}#dsp-key"),
            own_did,
            signing_key_bytes: signing_key.to_bytes().into(),
            public_key_xy: (x, y),
            insecure_http,
            required_scope,
        }
    }

    fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes((&self.signing_key_bytes).into()).expect("stored key bytes are always valid")
    }

    /// This connector's own DID document, served at `GET /dsp/did.json`
    /// so holders' Presentation APIs (via `SelfIssuedTokenVerifier`) can
    /// resolve the key that signs the re-packaged token this connector
    /// sends them.
    pub fn own_did_document(&self) -> Value {
        json!({
            "@context": ["https://www.w3.org/ns/did/v1"],
            "id": self.own_did,
            "verificationMethod": [{
                "id": self.own_key_id,
                "type": "JsonWebKey2020",
                "controller": self.own_did,
                "publicKeyJwk": {
                    "kty": "EC",
                    "crv": "P-256",
                    "x": URL_SAFE_NO_PAD.encode(self.public_key_xy.0),
                    "y": URL_SAFE_NO_PAD.encode(self.public_key_xy.1),
                }
            }],
            "service": [],
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidDocument {
    #[serde(default)]
    verification_method: Vec<VerificationMethod>,
    #[serde(default)]
    service: Vec<DidService>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerificationMethod {
    id: String,
    public_key_jwk: Option<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    x: String,
    y: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidService {
    #[serde(rename = "type")]
    ty: String,
    service_endpoint: String,
}

/// The caller identity and dataset entitlements a successful DCP
/// verification establishes - the DCP-mode equivalent of the bearer-mode
/// `caller` token in `authorize`/`visible_datasets` (`lib.rs`).
#[derive(Debug)]
pub struct VerifiedCaller {
    pub holder_did: String,
    pub catalog_access: HashSet<String>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before epoch").as_secs()
}

fn b64_decode(segment: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD.decode(segment).map_err(|e| format!("invalid base64url: {e}"))
}

/// Splits a compact JWS into (signing_input, header, payload) without
/// verifying anything - used to peek at `iss`/`kid` before we know which
/// key to verify against.
fn decode_jws_unverified(token: &str) -> Result<(String, Value, Value), String> {
    let mut parts = token.split('.');
    let header_b64 = parts.next().ok_or("missing JWS header")?;
    let payload_b64 = parts.next().ok_or("missing JWS payload")?;
    let _sig_b64 = parts.next().ok_or("missing JWS signature")?;
    if parts.next().is_some() {
        return Err("JWS has more than 3 segments".to_string());
    }
    let signing_input = format!("{header_b64}.{payload_b64}");
    let header: Value = serde_json::from_slice(&b64_decode(header_b64)?).map_err(|e| e.to_string())?;
    let payload: Value = serde_json::from_slice(&b64_decode(payload_b64)?).map_err(|e| e.to_string())?;
    Ok((signing_input, header, payload))
}

fn verify_jws_signature(token: &str, verifying_key: &VerifyingKey) -> Result<(), String> {
    let mut parts = token.split('.');
    let header_b64 = parts.next().ok_or("missing JWS header")?;
    let payload_b64 = parts.next().ok_or("missing JWS payload")?;
    let sig_b64 = parts.next().ok_or("missing JWS signature")?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    let sig_bytes = b64_decode(sig_b64)?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|e| format!("malformed signature: {e}"))?;
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|e| format!("signature verification failed: {e}"))
}

fn sign_jws(payload: &Value, signing_key: &SigningKey, kid: &str) -> String {
    let header = json!({"kid": kid, "alg": "ES256"});
    let header_b64 = URL_SAFE_NO_PAD.encode(header.to_string());
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
    let signing_input = format!("{header_b64}.{payload_b64}");
    let signature: Signature = signing_key.sign(signing_input.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    format!("{signing_input}.{sig_b64}")
}

/// did:web resolution per the (simplified) spec: `did:web:<host>[:<path
/// segments>]` -> `https://<host>/<path segments joined by "/">/did.json`,
/// or `https://<host>/.well-known/did.json` with no path segments.
fn did_web_to_url(did: &str, insecure_http: bool) -> Result<String, String> {
    let rest = did.strip_prefix("did:web:").ok_or("not a did:web DID")?;
    let mut segments = rest.split(':');
    let host = segments.next().ok_or("did:web has no host segment")?;
    let host = urlencoding_decode(host);
    let path_segments: Vec<String> = segments.map(urlencoding_decode).collect();
    let scheme = if insecure_http { "http" } else { "https" };
    if path_segments.is_empty() {
        Ok(format!("{scheme}://{host}/.well-known/did.json"))
    } else {
        Ok(format!("{scheme}://{host}/{}", path_segments.join("/")))
    }
}

fn urlencoding_decode(segment: &str) -> String {
    // did:web only ever percent-encodes ":" (as "%3A") in practice (to
    // embed a port number in the host segment) - a full percent-decoder
    // would be more correct but is unneeded machinery for this project's
    // one actual use case.
    segment.replace("%3A", ":").replace("%3a", ":")
}

async fn resolve_did(client: &reqwest::Client, did: &str, insecure_http: bool) -> Result<DidDocument, String> {
    let url = did_web_to_url(did, insecure_http)?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("failed to resolve DID {did} at {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("DID resolution for {did} returned HTTP {}", response.status()));
    }
    response
        .json::<DidDocument>()
        .await
        .map_err(|e| format!("DID document for {did} was not valid: {e}"))
}

fn find_verifying_key(doc: &DidDocument, kid: &str) -> Result<VerifyingKey, String> {
    let method = doc
        .verification_method
        .iter()
        .find(|m| m.id == kid)
        .ok_or_else(|| format!("no verification method '{kid}' in DID document"))?;
    let jwk = method
        .public_key_jwk
        .as_ref()
        .ok_or_else(|| format!("verification method '{kid}' has no publicKeyJwk"))?;
    jwk_to_verifying_key(jwk)
}

fn jwk_to_verifying_key(jwk: &Jwk) -> Result<VerifyingKey, String> {
    let x = b64_decode(&jwk.x)?;
    let y = b64_decode(&jwk.y)?;
    if x.len() != 32 || y.len() != 32 {
        return Err("EC JWK x/y must be 32 bytes for P-256".to_string());
    }
    let point = p256::EncodedPoint::from_affine_coordinates(
        p256::FieldBytes::from_slice(&x),
        p256::FieldBytes::from_slice(&y),
        false,
    );
    let public_key = p256::PublicKey::from_encoded_point(&point);
    let public_key = Option::<p256::PublicKey>::from(public_key).ok_or("invalid EC point in JWK")?;
    Ok(VerifyingKey::from(public_key))
}

fn credential_service_url(doc: &DidDocument) -> Result<String, String> {
    doc.service
        .iter()
        .find(|s| s.ty == "CredentialService")
        .map(|s| s.service_endpoint.clone())
        .ok_or_else(|| "DID document has no CredentialService entry".to_string())
}

#[derive(Debug, Serialize)]
struct PresentationQueryMessage {
    #[serde(rename = "@context")]
    context: &'static str,
    #[serde(rename = "@type")]
    ld_type: &'static str,
    scope: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PresentationResponseMessage {
    presentation: Vec<String>,
}

/// Verifies an incoming DCP self-issued bearer token per the flow
/// documented on this module, returning the caller's DID and the set of
/// dataset ids their presented credential(s) grant access to.
pub async fn verify_dcp_bearer_token(token: &str, config: &DcpConfig, http: &reqwest::Client) -> Result<VerifiedCaller, String> {
    let (signing_input_t1, header_t1, payload_t1) = decode_jws_unverified(token)?;
    let _ = signing_input_t1;
    let holder_did = payload_t1.get("iss").and_then(Value::as_str).ok_or("token has no iss")?.to_string();
    let kid_t1 = header_t1.get("kid").and_then(Value::as_str).ok_or("token has no kid")?;

    let holder_doc = resolve_did(http, &holder_did, config.insecure_http).await?;
    let holder_key = find_verifying_key(&holder_doc, kid_t1)?;
    verify_jws_signature(token, &holder_key)?;

    let aud = payload_t1.get("aud").and_then(Value::as_str).unwrap_or("");
    if aud != config.own_did {
        return Err(format!("token audience '{aud}' does not match this connector's DID '{}'", config.own_did));
    }

    let exp = payload_t1.get("exp").and_then(Value::as_u64).unwrap_or(0);
    if exp <= now_secs() {
        return Err("token has expired".to_string());
    }

    let nested_token = payload_t1
        .get("token")
        .and_then(Value::as_str)
        .ok_or("token has no nested presentation-access-token claim")?;

    // Step 3: proof of original possession - re-package the nested
    // token into a new self-issued token, signed with this connector's
    // own key, addressed back to the holder.
    let now = now_secs();
    let repackaged_payload = json!({
        "iss": config.own_did,
        "sub": config.own_did,
        "aud": holder_did,
        "token": nested_token,
        "iat": now,
        "nbf": now,
        "exp": now + 300,
        "jti": Uuid::new_v4().to_string(),
    });
    let repackaged_token = sign_jws(&repackaged_payload, &config.signing_key(), &config.own_key_id);

    let credential_service = credential_service_url(&holder_doc)?;
    let query_url = format!("{credential_service}/presentations/query");
    let query_body = PresentationQueryMessage {
        context: PRESENTATION_QUERY_CONTEXT,
        ld_type: "PresentationQueryMessage",
        scope: vec![config.required_scope.clone()],
    };
    let response = http
        .post(&query_url)
        .bearer_auth(&repackaged_token)
        .json(&query_body)
        .send()
        .await
        .map_err(|e| format!("presentation query to {query_url} failed: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("presentation query returned HTTP {status}: {body}"));
    }
    let presentation_response: PresentationResponseMessage =
        response.json().await.map_err(|e| format!("malformed presentation response: {e}"))?;
    let vp_jws = presentation_response.presentation.first().ok_or("presentation response had no presentation")?;

    // Step 5: verify the VP itself (signed by the holder).
    let (_, vp_header, vp_payload) = decode_jws_unverified(vp_jws)?;
    let vp_kid = vp_header.get("kid").and_then(Value::as_str).ok_or("VP has no kid")?;
    let vp_key = find_verifying_key(&holder_doc, vp_kid)?;
    verify_jws_signature(vp_jws, &vp_key)?;

    let vc_jws_list = vp_payload
        .get("vp")
        .and_then(|vp| vp.get("verifiableCredential"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if vc_jws_list.is_empty() {
        return Err("VerifiablePresentation contained no credentials".to_string());
    }

    let mut catalog_access = HashSet::new();
    for vc_value in &vc_jws_list {
        let vc_jws = vc_value.as_str().ok_or("verifiableCredential entry was not a string")?;
        let (_, vc_header, vc_payload) = decode_jws_unverified(vc_jws)?;
        let issuer_did = vc_payload.get("iss").and_then(Value::as_str).ok_or("VC has no iss")?.to_string();
        let vc_kid = vc_header.get("kid").and_then(Value::as_str).ok_or("VC has no kid")?;

        // Each credential may (in general) come from a different
        // issuer - resolve per credential rather than assuming they
        // share the holder's DID.
        let issuer_doc = resolve_did(http, &issuer_did, config.insecure_http).await?;
        let issuer_key = find_verifying_key(&issuer_doc, vc_kid)?;
        verify_jws_signature(vc_jws, &issuer_key)?;

        let vc_exp = vc_payload.get("exp").and_then(Value::as_u64).unwrap_or(0);
        if vc_exp <= now_secs() {
            continue; // expired credential: skip, don't grant its access
        }

        let vc_body = vc_payload.get("vc").cloned().unwrap_or(Value::Null);
        let types = vc_body.get("type").and_then(Value::as_array).cloned().unwrap_or_default();
        let has_expected_type = types.iter().any(|t| t.as_str() == Some(EXPECTED_CREDENTIAL_TYPE));
        if !has_expected_type {
            continue;
        }

        if let Some(access) = vc_body
            .get("credentialSubject")
            .and_then(|s| s.get("catalogAccess"))
            .and_then(Value::as_array)
        {
            for id in access {
                if let Some(id) = id.as_str() {
                    catalog_access.insert(id.to_string());
                }
            }
        }
    }

    Ok(VerifiedCaller { holder_did, catalog_access })
}
