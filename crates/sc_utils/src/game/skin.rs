use base64::prelude::{BASE64_STANDARD, BASE64_URL_SAFE_NO_PAD};
use base64::Engine;

/// Skin animation texture (Login ClientData.AnimatedImageData entry).
#[derive(Clone, Debug, Default)]
pub struct SkinAnimation {
    pub width: u32,
    pub height: u32,
    pub image: Vec<u8>,
    /// AnimatedTextureType (client Type field kept verbatim: 0=None, 1=Face, 2=Body32x32, 3=Body128x128).
    pub texture_type: i32,
    pub frames: f32,
    /// AnimationExpressionType (0=Linear, 1=Blinking).
    pub expression_type: i32,
}

/// Persona piece (Login ClientData.PersonaPieces entry).
#[derive(Clone, Debug, Default)]
pub struct PersonaPiece {
    pub piece_id: String,
    /// `"persona_"`-prefixed type string (e.g. persona_skeleton).
    pub piece_type: String,
    pub pack_id: String,
    pub is_default: bool,
    pub product_id: String,
}

/// Persona piece tint (Login ClientData.PieceTintColors entry).
#[derive(Clone, Debug, Default)]
pub struct PersonaTint {
    pub piece_type: String,
    /// `"#RRGGBB"` string list (protocol requires exactly 4 colors).
    pub colors: Vec<String>,
}

/// Player skin. Appearance fields beyond textures are kept at Login parse time and echoed 1:1
/// in PlayerList (dropping fields crashes real clients while they build the local player model).
#[derive(Clone, Debug, Default)]
pub struct Skin {
    skin_data: Vec<u8>,
    skin_id: String,
    width: u32,
    height: u32,
    // ===== Appearance fields required for complete echo (all decoded plaintext) =====
    pub play_fab_id: String,
    /// skinResourcePatch (JSON, includes geometry.default).
    pub skin_resource_patch: String,
    pub geometry_data: String,
    pub geometry_data_engine_version: String,
    pub animation_data: String,
    pub cape_id: String,
    pub cape_data: Vec<u8>,
    pub cape_width: u32,
    pub cape_height: u32,
    /// Echo falls back to skin_id when empty (vanilla server behavior).
    pub full_skin_id: String,
    /// "slim" / "wide".
    pub arm_size: String,
    /// `"#RRGGBB"` skin base color.
    pub skin_color: String,
    pub animations: Vec<SkinAnimation>,
    pub persona_pieces: Vec<PersonaPiece>,
    pub tint_colors: Vec<PersonaTint>,
    pub premium: bool,
    pub persona: bool,
    pub cape_on_classic: bool,
    pub overriding_player_appearance: bool,
}

impl Skin {
    pub const SINGLE_SKIN_SIZE: usize = 64 * 32 * 4;
    pub const DOUBLE_SKIN_SIZE: usize = 64 * 64 * 4;

    pub fn new(skin_data: Vec<u8>, width: u32, height: u32, skin_id: &str) -> Option<Self> {
        let expected_size = width.checked_mul(height)?.checked_mul(4)? as usize;
        if expected_size != skin_data.len() {
            return None;
        }
        Some(Self {
            skin_data,
            skin_id: skin_id.to_string(),
            width,
            height,
            ..Default::default()
        })
    }

    pub fn new_base64(base64: &str, width: u32, height: u32, skin_id: &str) -> Option<Self> {
        let base64_decode = BASE64_STANDARD.decode(base64).ok()?;
        Self::new(base64_decode, width, height, skin_id)
    }

    pub fn skin_id(&self) -> &str {
        &self.skin_id
    }

    pub fn skin_data(&self) -> &[u8] {
        &self.skin_data
    }

    pub fn dimensions(&self) -> (i32, i32) {
        (self.width as i32, self.height as i32)
    }

    /// Some JWT fields are base64-encoded strings (SkinResourcePatch / SkinGeometryData /
    /// SkinGeometryDataEngineVersion / SkinAnimationData), decoded to plaintext.
    pub fn decode_b64_claim(value: &str) -> String {
        if value.is_empty() {
            return String::new();
        }
        BASE64_STANDARD
            .decode(value)
            .or_else(|_| BASE64_URL_SAFE_NO_PAD.decode(value))
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default()
    }

    /// "#RRGGBB"/"RRGGBB" to ARGB i32.
    pub fn parse_hex_color(color: &str) -> i32 {
        if color.is_empty() {
            return 0;
        }
        let hex = color.strip_prefix('#').unwrap_or(color);
        i64::from_str_radix(hex, 16).map(|v| v as i32).unwrap_or(0)
    }
}
