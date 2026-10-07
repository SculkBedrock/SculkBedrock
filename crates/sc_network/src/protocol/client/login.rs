use crate::utils::auth::{self, AuthPayload, AuthType, ChainValidationResult, IdentityData};
use base64::prelude::{BASE64_STANDARD, BASE64_STANDARD_NO_PAD};
use base64::Engine;
use log::debug;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use sc_utils::game::skin::{PersonaPiece, PersonaTint, Skin, SkinAnimation};
use serde_json::Value;
use std::fmt::Display;
use std::io::Error;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Copy, Clone, Debug)]
pub enum LoginDevice {
    Unknown,
    Android,
    IOS,
    MacOS,
    FireOS,
    GearVR,
    HoloLens,
    Windows,
    Dedicated,
    TVOS,
    PlayStation,
    Switch,
    Xbox,
    WindowsPhone,
    Linux,
}

impl LoginDevice {
    pub fn index(&self) -> i32 {
        match self {
            LoginDevice::Unknown => 0,
            LoginDevice::Android => 1,
            LoginDevice::IOS => 2,
            LoginDevice::MacOS => 3,
            LoginDevice::FireOS => 4,
            LoginDevice::GearVR => 5,
            LoginDevice::HoloLens => 6,
            LoginDevice::Windows => 7,
            LoginDevice::Dedicated => 8,
            LoginDevice::TVOS => 9,
            LoginDevice::PlayStation => 10,
            LoginDevice::Switch => 11,
            LoginDevice::Xbox => 12,
            LoginDevice::WindowsPhone => 13,
            LoginDevice::Linux => 14,
        }
    }

    pub fn to_string(&self) -> &'static str {
        match self {
            LoginDevice::Unknown => "Unknown",
            LoginDevice::Android => "Android",
            LoginDevice::IOS => "iOS",
            LoginDevice::MacOS => "MacOS",
            LoginDevice::FireOS => "FireOS",
            LoginDevice::GearVR => "GearVR",
            LoginDevice::HoloLens => "HoloLens",
            LoginDevice::Windows => "Windows",
            LoginDevice::Dedicated => "Dedicated",
            LoginDevice::TVOS => "TVOS",
            LoginDevice::PlayStation => "PlayStation",
            LoginDevice::Switch => "Switch",
            LoginDevice::Xbox => "Xbox",
            LoginDevice::WindowsPhone => "WindowsPhone",
            LoginDevice::Linux => "Linux",
        }
    }

    pub fn from_u8(u8: u8) -> Self {
        match u8 {
            0 => LoginDevice::Unknown,
            1 => LoginDevice::Android,
            2 => LoginDevice::IOS,
            3 => LoginDevice::MacOS,
            4 => LoginDevice::FireOS,
            5 => LoginDevice::GearVR,
            6 => LoginDevice::HoloLens,
            7 => LoginDevice::Windows,
            8 => LoginDevice::Dedicated,
            9 => LoginDevice::TVOS,
            10 => LoginDevice::PlayStation,
            11 => LoginDevice::Switch,
            12 => LoginDevice::Xbox,
            13 => LoginDevice::WindowsPhone,
            14 => LoginDevice::Linux,
            _ => LoginDevice::Unknown,
        }
    }
}

impl Display for LoginDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl FromStr for LoginDevice {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Unknown" => Ok(LoginDevice::Unknown),
            "Android" => Ok(LoginDevice::Android),
            "iOS" => Ok(LoginDevice::IOS),
            "MacOS" => Ok(LoginDevice::MacOS),
            "FireOS" => Ok(LoginDevice::FireOS),
            "GearVR" => Ok(LoginDevice::GearVR),
            "HoloLens" => Ok(LoginDevice::HoloLens),
            "Windows" => Ok(LoginDevice::Windows),
            "Dedicated" => Ok(LoginDevice::Dedicated),
            "TVOS" => Ok(LoginDevice::TVOS),
            "PlayStation" => Ok(LoginDevice::PlayStation),
            "Switch" => Ok(LoginDevice::Switch),
            "Xbox" => Ok(LoginDevice::Xbox),
            "WindowsPhone" => Ok(LoginDevice::WindowsPhone),
            "Linux" => Ok(LoginDevice::Linux),
            _ => Ok(LoginDevice::Unknown),
        }
    }
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct Login {
    pub username: String,
    pub protocol: u32,
    pub identity: Uuid,
    pub identity_public_key: String,
    pub client_id: i64,
    pub skin: Skin,
    pub issue_unix_time: i64,
    pub device_os: u8,
    pub device_model: String,
    pub language_code: String,
    pub game_version: String,
    pub server_address: String,
    /// Authentication type
    pub auth_type: AuthType,
    /// Whether Mojang signature verification passed (quick check)
    pub signed: bool,
    /// Raw auth payload for later async verification (Token Auth and Chain)
    pub auth_payload: AuthPayload,
    /// XUID (extracted from Token Auth or Chain)
    pub xuid: String,
}

impl Login {
    /// Asynchronously verify the login signature.
    /// Call after Reader::read; performs full signature verification
    /// for Token Auth (JWKS) and Certificate Chain (Mojang signatures).
    pub async fn validate(&mut self) -> Result<ChainValidationResult, Error> {
        let payload = self.auth_payload.clone();
        let result = auth::validate_payload(payload).await?;
        // Update identity from verified data.
        self.username = result.identity.display_name.clone();
        self.identity = result.identity.identity;
        self.identity_public_key = result.identity.identity_public_key.clone();
        self.xuid = result.identity.xuid.clone();
        self.signed = result.signed;
        Ok(result)
    }
}

impl Reader<Login> for Login {
    fn read(buf: &mut ByteReader) -> Result<Login, Error> {
        let mut protocol = buf.read_u32()?;
        if protocol == 0 {
            buf.read_u16()?;
            protocol = buf.read_u32()?;
        }

        let buf = buf.read_sized_slice()?;
        let mut buf = ByteReader::from(buf);

        // Parse chain/auth data.
        let len = buf.read_u32_le()? as usize;
        let chain_data = buf.read_bytes(len)?;
        let chain_data_str = String::from_utf8_lossy(&chain_data).to_string();

        debug!("Login >> Auth data: {}", &chain_data_str);

        // Parse the outer JSON with the auth module.
        let auth_payload = auth::parse_auth_wrapper(&chain_data_str)?;
        let auth_type = auth_payload.auth_type();

        debug!("Login >> Auth type: {:?}", auth_type);

        // Extract identity data synchronously (no signature check).
        let identity_data = match &auth_payload {
            AuthPayload::Token { token, .. } => {
                // Token Auth: xname, xid, cpk from JWT claims.
                let claims = decode_jwt_payload(token, 1)
                    .ok_or(Error::other("Cannot decode Token JWT payload"))?;
                extract_token_claims(&claims)?
            }
            AuthPayload::Certificate { chain, .. } => {
                // Certificate Chain: payload of the last JWT.
                let last_jwt = chain
                    .last()
                    .ok_or(Error::other("Empty certificate chain"))?;
                let payload = decode_jwt_payload(last_jwt, 1)
                    .ok_or(Error::other("Cannot decode chain JWT payload"))?;
                extract_chain_claims(&payload)?
            }
        };

        let username = identity_data.display_name;
        let identity = identity_data.identity;
        let identity_public_key = identity_data.identity_public_key;
        let xuid = identity_data.xuid;

        // Keep the raw AuthPayload for later async verification.
        let auth_payload_clone = auth_payload.clone();

        // Extract issue time.
        let issue_unix_time = match &auth_payload {
            auth::AuthPayload::Certificate { chain, .. } => chain
                .last()
                .and_then(|jwt| {
                    decode_jwt_payload(jwt, 1)
                        .and_then(|p| p.get("iat").and_then(|v| v.as_i64()).map(|t| t * 1000))
                })
                .unwrap_or(-1),
            auth::AuthPayload::Token { token, .. } => decode_jwt_payload(token, 1)
                .and_then(|p| p.get("iat").and_then(|v| v.as_i64()).map(|t| t * 1000))
                .unwrap_or(-1),
        };

        // Signed flag: assumed signed for Token Auth with Full auth type;
        // real verification happens asynchronously in validate().
        let signed = auth_type == AuthType::Full;

        // Parse skin data.
        let mut client_id: i64 = 0;
        let mut device_os: u8 = 0;
        let mut device_model = String::new();
        let mut language_code = String::new();
        let mut game_version = String::new();
        let mut server_address = String::new();
        let mut skin = None;

        let len = buf.read_u32_le()? as usize;
        let skin_data = buf.read_bytes(len)?;
        let skin_data_str = String::from_utf8_lossy(&skin_data).to_string();

        if let Some(skin_token) = decode_jwt_payload(&skin_data_str, 1) {
            if let Some(id) = skin_token.get("ClientRandomId") {
                client_id = id.as_i64().unwrap_or(0);
            }
            if let Some(os) = skin_token.get("DeviceOS") {
                device_os = os.as_u64().unwrap_or(0) as u8;
            }
            if let Some(version) = skin_token.get("GameVersion") {
                game_version = version.as_str().unwrap_or("").to_string();
            }
            if let Some(model) = skin_token.get("DeviceModel") {
                device_model = model.as_str().unwrap_or("").to_string();
            }
            if let Some(code) = skin_token.get("LanguageCode") {
                language_code = code.as_str().unwrap_or("").to_string();
            }
            if let Some(address) = skin_token.get("ServerAddress") {
                server_address = address.as_str().unwrap_or("").to_string();
            }
            let skin_width = skin_token
                .get("SkinImageWidth")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let skin_height = skin_token
                .get("SkinImageHeight")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            if let Some(skin_data_b64) = skin_token.get("SkinData").and_then(|v| v.as_str()) {
                if let Some(skin_id) = skin_token.get("SkinId").and_then(|v| v.as_str()) {
                    skin = Skin::new_base64(skin_data_b64, skin_width, skin_height, skin_id).map(
                        |mut s| {
                            // Keep all appearance fields for 1:1 PlayerList echo;
                            // missing fields crash real clients.
                            s.play_fab_id = skin_token
                                .get("PlayFabId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            s.skin_resource_patch = Skin::decode_b64_claim(
                                skin_token
                                    .get("SkinResourcePatch")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                            );
                            s.geometry_data = Skin::decode_b64_claim(
                                skin_token
                                    .get("SkinGeometryData")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                            );
                            s.geometry_data_engine_version = Skin::decode_b64_claim(
                                skin_token
                                    .get("SkinGeometryDataEngineVersion")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                            );
                            s.animation_data = Skin::decode_b64_claim(
                                skin_token
                                    .get("SkinAnimationData")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                            );
                            s.cape_id = skin_token
                                .get("CapeId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Some(cape_b64) =
                                skin_token.get("CapeData").and_then(|v| v.as_str())
                            {
                                if let Some(cape) = BASE64_STANDARD.decode(cape_b64).ok() {
                                    s.cape_width = skin_token
                                        .get("CapeImageWidth")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                    s.cape_height = skin_token
                                        .get("CapeImageHeight")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                    s.cape_data = cape;
                                }
                            }
                            s.arm_size = skin_token
                                .get("ArmSize")
                                .and_then(|v| v.as_str())
                                .unwrap_or("wide")
                                .to_string();
                            s.skin_color = skin_token
                                .get("SkinColor")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            s.premium = skin_token
                                .get("PremiumSkin")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            s.persona = skin_token
                                .get("PersonaSkin")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            s.cape_on_classic = skin_token
                                .get("CapeOnClassicSkin")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            s.overriding_player_appearance = skin_token
                                .get("OverrideSkin")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if let Some(arr) = skin_token
                                .get("AnimatedImageData")
                                .and_then(|v| v.as_array())
                            {
                                for a in arr {
                                    let image =
                                        a.get("Image").and_then(|v| v.as_str()).unwrap_or("");
                                    let decoded = BASE64_STANDARD.decode(image).unwrap_or_default();
                                    s.animations.push(SkinAnimation {
                                        width: a
                                            .get("ImageWidth")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                        height: a
                                            .get("ImageHeight")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                        image: decoded,
                                        texture_type: a
                                            .get("Type")
                                            .and_then(|v| v.as_i64())
                                            .unwrap_or(0)
                                            as i32,
                                        frames: a
                                            .get("Frames")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0)
                                            as f32,
                                        expression_type: a
                                            .get("AnimationExpression")
                                            .and_then(|v| v.as_i64())
                                            .unwrap_or(0)
                                            as i32,
                                    });
                                }
                            }
                            if let Some(arr) =
                                skin_token.get("PersonaPieces").and_then(|v| v.as_array())
                            {
                                for p in arr {
                                    s.persona_pieces.push(PersonaPiece {
                                        piece_id: p
                                            .get("PieceId")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        piece_type: p
                                            .get("PieceType")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        pack_id: p
                                            .get("PackId")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        is_default: p
                                            .get("IsDefault")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        product_id: p
                                            .get("ProductId")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                    });
                                }
                            }
                            if let Some(arr) =
                                skin_token.get("PieceTintColors").and_then(|v| v.as_array())
                            {
                                for t in arr {
                                    let colors = t
                                        .get("Colors")
                                        .and_then(|v| v.as_array())
                                        .map(|cs| {
                                            cs.iter()
                                                .filter_map(|c| c.as_str().map(String::from))
                                                .collect::<Vec<_>>()
                                        })
                                        .unwrap_or_default();
                                    s.tint_colors.push(PersonaTint {
                                        piece_type: t
                                            .get("PieceType")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        colors,
                                    });
                                }
                            }
                            s
                        },
                    );
                }
            }
        }

        let skin = skin.ok_or(Error::other("Invalid skin"))?;

        Ok(Self {
            username,
            protocol,
            identity,
            identity_public_key,
            client_id,
            skin,
            issue_unix_time,
            device_os,
            device_model,
            language_code,
            game_version,
            server_address,
            auth_type,
            signed,
            auth_payload: auth_payload_clone,
            xuid,
        })
    }
}

impl Writer for Login {
    fn write(&self, _buf: &mut ByteWriter) -> Result<(), Error> {
        Ok(())
    }
}

// Helpers.

/// Decode one JWT section (0=header, 1=payload).
fn decode_jwt_payload(jwt_str: &str, index: usize) -> Option<Value> {
    let parts: Vec<&str> = jwt_str.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let decoded = BASE64_STANDARD_NO_PAD
        .decode(parts[index])
        .or_else(|_| base64::prelude::BASE64_URL_SAFE_NO_PAD.decode(parts[index]))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Identity data from Token Auth JWT claims.
fn extract_token_claims(claims: &Value) -> Result<IdentityData, Error> {
    let claims = claims
        .as_object()
        .ok_or(Error::other("Token claims is not an object"))?;

    let display_name = claims
        .get("xname")
        .and_then(|v| v.as_str())
        .ok_or(Error::other("Missing 'xname' in token claims"))?
        .to_string();

    let xuid = claims
        .get("xid")
        .and_then(|v| v.as_str())
        .ok_or(Error::other("Missing 'xid' in token claims"))?
        .to_string();

    let identity_public_key = claims
        .get("cpk")
        .and_then(|v| v.as_str())
        .ok_or(Error::other("Missing 'cpk' in token claims"))?
        .to_string();

    // UUID derives from the XUID.
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

/// Identity data from a Certificate Chain JWT payload (legacy format).
fn extract_chain_claims(payload: &Value) -> Result<IdentityData, Error> {
    let payload = payload
        .as_object()
        .ok_or(Error::other("Chain payload is not an object"))?;

    let identity_public_key = payload
        .get("identityPublicKey")
        .or_else(|| payload.get("clientPublicKey"))
        .or_else(|| payload.get("cpk"))
        .and_then(|v| v.as_str())
        .ok_or(Error::other("Missing identity public key in chain"))?
        .to_string();

    let extra_data = payload
        .get("extraData")
        .and_then(|v| v.as_object())
        .ok_or(Error::other("Missing extraData in chain payload"))?;

    let display_name = extra_data
        .get("displayName")
        .or_else(|| extra_data.get("xname"))
        .and_then(|v| v.as_str())
        .ok_or(Error::other("Missing displayName in extraData"))?
        .to_string();

    // Try several candidate fields for identity.
    let identity_str = extra_data
        .get("identity")
        .or_else(|| extra_data.get("xuid"))
        .or_else(|| extra_data.get("xid"))
        .or_else(|| extra_data.get("XUID"))
        .and_then(|v| v.as_str())
        .ok_or(Error::other("Missing identity in extraData"))?;

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

/// Derive a UUID from an XUID (UUID v3, MD5-based).
fn uuid_from_xuid(xuid: &str) -> Uuid {
    let data = format!("pocket-auth-1-xuid:{}", xuid);
    Uuid::new_v3(&Uuid::NAMESPACE_OID, data.as_bytes())
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetLocalPlayerAsInitialized {
    pub entity_runtime_id: u64,
}

impl sc_binary::interfaces::Reader<SetLocalPlayerAsInitialized> for SetLocalPlayerAsInitialized {
    fn read(buf: &mut sc_binary::ByteReader) -> Result<Self, Error> {
        Ok(Self {
            entity_runtime_id: buf.read_var_u64()?,
        })
    }
}

impl sc_binary::interfaces::Writer for SetLocalPlayerAsInitialized {
    fn write(&self, buf: &mut sc_binary::ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_runtime_id)
    }
}
