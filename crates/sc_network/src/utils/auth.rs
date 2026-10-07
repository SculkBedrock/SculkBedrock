use base64::prelude::BASE64_STANDARD;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use log::debug;
use serde::Deserialize;
use serde_json::Value;
use std::io;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Auth type.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AuthType {
    /// Default for legacy clients.
    Unknown,
    /// Authenticated directly via the Mojang auth server.
    Full,
    /// Split-screen player using host auth.
    Guest,
    /// Unauthenticated (offline mode/self-signed).
    SelfSigned,
}

impl AuthType {
    /// Map the AuthenticationType field from the JWT payload.
    pub fn from_auth_type_ordinal(ordinal: i64) -> Result<Self, io::Error> {
        match ordinal {
            0 => Ok(AuthType::Full),
            1 => Ok(AuthType::Guest),
            2 => Ok(AuthType::SelfSigned),
            _ => Err(io::Error::other(format!(
                "Invalid AuthenticationType ordinal: {}",
                ordinal
            ))),
        }
    }

    pub fn is_authenticated(&self) -> bool {
        matches!(self, AuthType::Full)
    }
}

/// Auth payload type.
#[derive(Clone, Debug)]
pub enum AuthPayload {
    /// Token Auth mode: contains the JWT token directly.
    Token { token: String, auth_type: AuthType },
    /// Certificate Chain mode: contains a JWT chain array.
    Certificate {
        chain: Vec<String>,
        auth_type: AuthType,
    },
}

impl AuthPayload {
    pub fn auth_type(&self) -> AuthType {
        match self {
            AuthPayload::Token { auth_type, .. } => *auth_type,
            AuthPayload::Certificate { auth_type, .. } => *auth_type,
        }
    }
}

/// Identity verification result.
#[derive(Clone, Debug)]
pub struct IdentityData {
    pub display_name: String,
    pub identity: Uuid,
    pub xuid: String,
    pub identity_public_key: String,
    pub title_id: Option<String>,
    pub minecraft_id: Option<String>,
}

/// Chain verification result.
#[derive(Clone, Debug)]
pub struct ChainValidationResult {
    pub signed: bool,
    pub identity: IdentityData,
}

// JWKS types.

#[derive(Debug, Clone, Deserialize)]
pub struct JwkKey {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
    pub kid: Option<String>,
    #[serde(rename = "use")]
    pub use_: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwksResponse {
    pub keys: Vec<JwkKey>,
}

// Discovery and OpenID configuration.

const DISCOVERY_ENDPOINT: &str =
    "https://client.discovery.minecraft-services.net/api/v1.0/discovery/MinecraftPE/builds/1.0.0.0";

lazy_static::lazy_static! {
    static ref AUTH_ENV: Arc<RwLock<Option<AuthEnvironment>>> = Arc::new(RwLock::new(None));
}

struct AuthEnvironment {
    jwks_url: String,
    issuer: String,
    jwks: JwksResponse,
}

impl AuthEnvironment {
    /// Initialize the auth environment: fetch discovery data, OpenID config, then JWKS.
    pub async fn init() -> Result<Self, io::Error> {
        debug!("Fetching discovery data from {}", DISCOVERY_ENDPOINT);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| io::Error::other(format!("Failed to create HTTP client: {}", e)))?;

        // 1. Fetch discovery data.
        let discovery: Value = client
            .get(DISCOVERY_ENDPOINT)
            .send()
            .await
            .map_err(|e| io::Error::other(format!("Failed to fetch discovery data: {}", e)))?
            .json()
            .await
            .map_err(|e| io::Error::other(format!("Failed to parse discovery data: {}", e)))?;

        // 2. Extract serviceUri.
        let service_uri = discovery
            .get("result")
            .and_then(|r| r.get("serviceEnvironments"))
            .and_then(|e| e.get("auth"))
            .and_then(|a| a.get("prod"))
            .and_then(|p| p.get("serviceUri"))
            .and_then(|u| u.as_str())
            .ok_or_else(|| io::Error::other("Missing serviceUri in discovery data"))?;

        // 3. Fetch the OpenID configuration.
        let openid_url = format!("{}/.well-known/openid-configuration", service_uri);
        debug!("Fetching OpenID configuration from {}", openid_url);

        let openid: Value = client
            .get(&openid_url)
            .send()
            .await
            .map_err(|e| io::Error::other(format!("Failed to fetch OpenID config: {}", e)))?
            .json()
            .await
            .map_err(|e| io::Error::other(format!("Failed to parse OpenID config: {}", e)))?;

        let jwks_url = openid
            .get("jwks_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io::Error::other("Missing jwks_uri in OpenID config"))?
            .to_string();

        let issuer = openid
            .get("issuer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io::Error::other("Missing issuer in OpenID config"))?
            .to_string();

        // 4. Fetch JWKS.
        debug!("Fetching JWKS from {}", jwks_url);
        let jwks: JwksResponse = client
            .get(&jwks_url)
            .send()
            .await
            .map_err(|e| io::Error::other(format!("Failed to fetch JWKS: {}", e)))?
            .json()
            .await
            .map_err(|e| io::Error::other(format!("Failed to parse JWKS: {}", e)))?;

        debug!("Auth environment initialized with {} keys", jwks.keys.len());

        Ok(Self {
            jwks_url,
            issuer,
            jwks,
        })
    }

    /// Find a public key in JWKS by key ID.
    fn find_key(&self, kid: Option<&str>) -> Option<&JwkKey> {
        if let Some(kid) = kid {
            self.jwks
                .keys
                .iter()
                .find(|k| k.kid.as_deref() == Some(kid))
        } else {
            self.jwks.keys.first()
        }
    }
}

/// Initialize the global auth environment (call at server startup).
pub async fn init_auth_environment() -> Result<(), io::Error> {
    let env = AuthEnvironment::init().await?;
    let mut guard = AUTH_ENV.write().await;
    *guard = Some(env);
    Ok(())
}

// Token verification.

/// Convert the EC public key (x, y) in a JWK to X.509 SubjectPublicKeyInfo DER.
fn jwk_to_ec_der(jwk: &JwkKey) -> Result<Vec<u8>, io::Error> {
    let x_bytes = BASE64_URL_SAFE_NO_PAD
        .decode(&jwk.x)
        .or_else(|_| BASE64_STANDARD.decode(&jwk.x))
        .map_err(|e| io::Error::other(format!("Failed to decode JWK x: {}", e)))?;
    let y_bytes = BASE64_URL_SAFE_NO_PAD
        .decode(&jwk.y)
        .or_else(|_| BASE64_STANDARD.decode(&jwk.y))
        .map_err(|e| io::Error::other(format!("Failed to decode JWK y: {}", e)))?;

    // SEC1 uncompressed public key: 04 || x || y.
    let mut point = Vec::with_capacity(1 + x_bytes.len() + y_bytes.len());
    point.push(0x04);
    point.extend_from_slice(&x_bytes);
    point.extend_from_slice(&y_bytes);

    // X.509 SubjectPublicKeyInfo DER prefix (P-384).
    // SEQUENCE {
    //   SEQUENCE { OID 1.2.840.10045.2.1 (ecPublicKey), OID 1.3.132.0.34 (secp384r1) }
    //   BIT STRING (uncompressed point)
    // }
    let algorithm_prefix: &[u8] = &[
        0x30, 0x10, // SEQUENCE (16 bytes)
        0x06, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01, // OID: ecPublicKey
        0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x22, // OID: secp384r1
    ];

    let bit_string_len = 1 + point.len(); // 0x00 unused-bits byte + point
    let inner_len = algorithm_prefix.len() + 2 + bit_string_len; // +2 for BIT STRING tag+len

    let mut der = Vec::with_capacity(2 + inner_len);
    der.push(0x30); // SEQUENCE
    der.push(inner_len as u8);
    der.extend_from_slice(algorithm_prefix);
    der.push(0x03); // BIT STRING
    der.push(bit_string_len as u8);
    der.push(0x00); // unused bits
    der.extend_from_slice(&point);

    Ok(der)
}

/// Verify a Token Auth JWT.
pub async fn validate_token(
    auth_type: AuthType,
    token: &str,
) -> Result<ChainValidationResult, io::Error> {
    match auth_type {
        AuthType::Full => validate_token_online(token).await,
        AuthType::SelfSigned | AuthType::Guest => validate_token_offline(token).await,
        AuthType::Unknown => Err(io::Error::other(
            "Cannot validate token with UNKNOWN auth type",
        )),
    }
}

/// Online verification: verify the JWT signature with JWKS.
async fn validate_token_online(token: &str) -> Result<ChainValidationResult, io::Error> {
    // Load the auth environment.
    let env_guard = AUTH_ENV.read().await;
    let env = env_guard
        .as_ref()
        .ok_or_else(|| io::Error::other("Auth environment not initialized"))?;

    // Parse the header for kid.
    let header = decode_header(token)
        .map_err(|e| io::Error::other(format!("Failed to decode JWT header: {}", e)))?;

    let kid = header.kid.as_deref();

    // Find the matching public key in JWKS.
    let jwk = env
        .find_key(kid)
        .ok_or_else(|| io::Error::other("No matching key found in JWKS"))?;

    // Convert the JWK to a DER-encoded EC public key.
    let ec_der = jwk_to_ec_der(jwk)?;
    let decoding_key = DecodingKey::from_ec_der(&ec_der);

    // Configure validation parameters.
    let mut validation = Validation::new(Algorithm::ES384);
    validation.set_audience(&["api://auth-minecraft-services/multiplayer"]);
    validation.set_issuer(&[&env.issuer]);
    validation.validate_exp = true;
    validation.required_spec_claims.insert("sub".to_string());

    // Verify and parse the JWT.
    let token_data = decode::<Value>(token, &decoding_key, &validation)
        .map_err(|e| io::Error::other(format!("JWT validation failed: {}", e)))?;

    // Extract identity data from claims.
    let claims = &token_data.claims;
    let identity = extract_identity_from_token_claims(claims)?;

    debug!(
        "Token validated successfully for: {}",
        identity.display_name
    );

    Ok(ChainValidationResult {
        signed: true,
        identity,
    })
}

/// Offline verification: skip signature verification, only parse claims.
async fn validate_token_offline(token: &str) -> Result<ChainValidationResult, io::Error> {
    // Decode the header for the algorithm.
    let header = decode_header(token)
        .map_err(|e| io::Error::other(format!("Failed to decode JWT header: {}", e)))?;

    // Skip signature verification.
    let mut validation = Validation::new(header.alg);
    validation.insecure_disable_signature_validation();
    validation.validate_exp = false;
    validation.validate_aud = false;

    let token_data = decode::<Value>(token, &DecodingKey::from_secret(&[]), &validation)
        .map_err(|e| io::Error::other(format!("Failed to decode token: {}", e)))?;

    let claims = &token_data.claims;
    let identity = extract_identity_from_token_claims(claims)?;

    Ok(ChainValidationResult {
        signed: false,
        identity,
    })
}

/// Extract identity data from Token JWT claims.
fn extract_identity_from_token_claims(claims: &Value) -> Result<IdentityData, io::Error> {
    let claims = claims
        .as_object()
        .ok_or_else(|| io::Error::other("Token claims is not an object"))?;

    let display_name = claims
        .get("xname")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::other("Missing 'xname' in token claims"))?
        .to_string();

    let xuid = claims
        .get("xid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::other("Missing 'xid' in token claims"))?
        .to_string();

    let identity_public_key = claims
        .get("cpk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::other("Missing 'cpk' in token claims"))?
        .to_string();

    // Derive the UUID from the XUID (UUID v3, MD5-based).
    let identity = uuid_from_xuid(&xuid);

    let title_id = claims
        .get("titleId")
        .and_then(|v| v.as_str())
        .map(String::from);
    let minecraft_id = claims.get("mid").and_then(|v| v.as_str()).map(String::from);

    Ok(IdentityData {
        display_name,
        identity,
        xuid,
        identity_public_key,
        title_id,
        minecraft_id,
    })
}

/// Derive a UUID from an XUID (UUID v3, MD5-based).
fn uuid_from_xuid(xuid: &str) -> Uuid {
    let data = format!("pocket-auth-1-xuid:{}", xuid);
    Uuid::new_v3(&Uuid::NAMESPACE_OID, data.as_bytes())
}

// Chain verification (legacy compatibility).

/// Verify a Certificate Chain.
pub fn validate_chain(chain: &[String]) -> Result<ChainValidationResult, io::Error> {
    match chain.len() {
        1 => validate_chain_offline(&chain[0]),
        3 => validate_chain_online(chain),
        n => Err(io::Error::other(format!("Unexpected chain length: {}", n))),
    }
}

/// Single-JWT chain (offline/proxy mode).
fn validate_chain_offline(jwt_str: &str) -> Result<ChainValidationResult, io::Error> {
    let payload = decode_jwt_payload(jwt_str, 1)?;
    let identity = extract_identity_from_chain_payload(&payload)?;

    Ok(ChainValidationResult {
        signed: false,
        identity,
    })
}

/// Three-JWT chain (online mode).
fn validate_chain_online(chain: &[String]) -> Result<ChainValidationResult, io::Error> {
    use crate::utils::encryption::MinecraftEncryption;

    // Verify the chain signatures.
    let mut current_key_der: Option<Vec<u8>> = None;
    let mut last_payload: Option<Value> = None;
    let mut is_signed = false;

    for (i, jwt_str) in chain.iter().enumerate() {
        // Parse the header.
        let header = decode_jwt_payload(jwt_str, 0)?;
        let x5u = header
            .get("x5u")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io::Error::other("Missing x5u in chain header"))?;

        let expected_key = BASE64_STANDARD
            .decode(x5u)
            .map_err(|e| io::Error::other(format!("Failed to decode x5u: {}", e)))?;

        // Check chain continuity.
        if let Some(ref key) = current_key_der {
            if key != &expected_key {
                return Err(io::Error::other("Received broken chain"));
            }
        }
        current_key_der = Some(expected_key.clone());

        // Verify the signature with the current public key.
        let decoding_key = DecodingKey::from_ec_der(&expected_key);
        let mut validation = Validation::new(Algorithm::ES384);
        validation.validate_exp = false;
        validation.validate_aud = false;

        match decode::<Value>(jwt_str, &decoding_key, &validation) {
            Ok(token_data) => {
                last_payload = Some(token_data.claims);
            }
            Err(_) => {
                return Err(io::Error::other(format!(
                    "Chain signature verification failed at index {}",
                    i
                )));
            }
        }

        // The second chain entry must be signed by Mojang.
        if i == 1 {
            if x5u == MinecraftEncryption::MOJANG_PUBLIC_KEY {
                is_signed = true;
            } else {
                return Err(io::Error::other("The chain isn't signed by Mojang!"));
            }
        }

        // Extract the next public key from the payload.
        let payload = decode_jwt_payload(jwt_str, 1)?;
        if let Some(next_key) = payload.get("identityPublicKey").and_then(|v| v.as_str()) {
            current_key_der = Some(BASE64_STANDARD.decode(next_key).map_err(|e| {
                io::Error::other(format!("Failed to decode identityPublicKey: {}", e))
            })?);
        }
    }

    // Extract identity data from the last payload.
    let payload = last_payload.ok_or_else(|| io::Error::other("No payload found in chain"))?;
    let identity = extract_identity_from_chain_payload(&payload)?;

    Ok(ChainValidationResult {
        signed: is_signed,
        identity,
    })
}

/// Extract identity data from a Chain payload (legacy format).
fn extract_identity_from_chain_payload(payload: &Value) -> Result<IdentityData, io::Error> {
    let payload = payload
        .as_object()
        .ok_or_else(|| io::Error::other("Chain payload is not an object"))?;

    let identity_public_key = payload
        .get("identityPublicKey")
        .or_else(|| payload.get("clientPublicKey"))
        .or_else(|| payload.get("cpk"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::other("Missing identity public key in chain"))?
        .to_string();

    let extra_data = payload
        .get("extraData")
        .and_then(|v| v.as_object())
        .ok_or_else(|| io::Error::other("Missing extraData in chain payload"))?;

    let display_name = extra_data
        .get("displayName")
        .or_else(|| extra_data.get("xname"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::other("Missing displayName in extraData"))?
        .to_string();

    let identity_str = extra_data
        .get("identity")
        .or_else(|| extra_data.get("xuid"))
        .or_else(|| extra_data.get("xid"))
        .or_else(|| extra_data.get("XUID"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::other("Missing identity in extraData"))?;

    // Try to parse as UUID; derive from xuid when invalid.
    let identity = Uuid::parse_str(identity_str).unwrap_or_else(|_| uuid_from_xuid(identity_str));

    let xuid = extra_data
        .get("XUID")
        .or_else(|| extra_data.get("xuid"))
        .or_else(|| extra_data.get("xid"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let title_id = extra_data
        .get("titleId")
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(IdentityData {
        display_name,
        identity,
        xuid,
        identity_public_key,
        title_id,
        minecraft_id: None,
    })
}

/// Parse the outer auth JSON into an AuthPayload.
pub fn parse_auth_wrapper(json_str: &str) -> Result<AuthPayload, io::Error> {
    let map: Value = serde_json::from_str(json_str)
        .map_err(|e| io::Error::other(format!("Failed to parse auth JSON: {}", e)))?;
    let map = map
        .as_object()
        .ok_or_else(|| io::Error::other("Auth data is not a JSON object"))?;

    // New format: has an AuthenticationType field.
    if let Some(auth_type_val) = map.get("AuthenticationType") {
        let ordinal = auth_type_val
            .as_i64()
            .ok_or_else(|| io::Error::other("AuthenticationType is not a number"))?;
        let auth_type = AuthType::from_auth_type_ordinal(ordinal)?;

        // Prefer the Token field.
        if let Some(token) = map.get("Token").and_then(|v| v.as_str()) {
            if !token.is_empty() {
                return Ok(AuthPayload::Token {
                    token: token.to_string(),
                    auth_type,
                });
            }
        }

        // Check the Certificate field.
        if let Some(cert_json) = map.get("Certificate").and_then(|v| v.as_str()) {
            if !cert_json.is_empty() {
                let cert_data: Value = serde_json::from_str(cert_json).map_err(|e| {
                    io::Error::other(format!("Failed to parse Certificate JSON: {}", e))
                })?;
                let chain: Vec<String> = cert_data
                    .get("chain")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| io::Error::other("Certificate missing 'chain' field"))?
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                return Ok(AuthPayload::Certificate { chain, auth_type });
            }
        }

        return Err(io::Error::other(
            "AuthPayload has neither Token nor Certificate",
        ));
    }

    // Legacy format: a chain array directly.
    if let Some(chains) = map.get("chain") {
        let chain: Vec<String> = chains
            .as_array()
            .ok_or_else(|| io::Error::other("'chain' is not an array"))?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        return Ok(AuthPayload::Certificate {
            chain,
            auth_type: AuthType::Unknown,
        });
    }

    Err(io::Error::other(
        "Invalid auth data format: no AuthenticationType or chain",
    ))
}

/// Verify an AuthPayload and return identity data.
pub async fn validate_payload(payload: AuthPayload) -> Result<ChainValidationResult, io::Error> {
    match payload {
        AuthPayload::Token { token, auth_type } => validate_token(auth_type, &token).await,
        AuthPayload::Certificate {
            chain,
            auth_type: _,
        } => validate_chain(&chain),
    }
}

// Helpers.

/// Decode the given part of a JWT (0=header, 1=payload).
fn decode_jwt_payload(jwt_str: &str, index: usize) -> Result<Value, io::Error> {
    let parts: Vec<&str> = jwt_str.split('.').collect();
    if parts.len() < 2 {
        return Err(io::Error::other("Invalid JWT format"));
    }
    let decoded = BASE64_STANDARD
        .decode(parts[index])
        .or_else(|_| {
            // Try URL-safe base64.
            base64::prelude::BASE64_URL_SAFE_NO_PAD.decode(parts[index])
        })
        .map_err(|e| io::Error::other(format!("Failed to decode JWT part: {}", e)))?;
    serde_json::from_slice(&decoded)
        .map_err(|e| io::Error::other(format!("Failed to parse JWT JSON: {}", e)))
}
