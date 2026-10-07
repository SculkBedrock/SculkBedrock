//! Minecraft packet encryption (pure Rust).
//!
//! - Key exchange: P-384 ECDH (`p384` crate).
//! - Data encryption: AES-256.
//!   - Protocol >= 428: GCM streaming: Bedrock uses AES-GCM as a continuous CTR stream
//!     (no auth tag); the keystream starts at `[iv12 || be32(2)]` and increments the low 32 bits.
//!     It behaves as a 12-byte IV GCM stream.
//!   - Protocol < 428: CFB8 with byte-wise feedback.
//! - Checksum: SHA-256 (`sha2`).
//! - Handshake JWT (ES384): `jsonwebtoken`.
//!
//! Encryption is microsecond-scale pure CPU work (no IO), so it stays synchronous
//! and is called directly inside lock critical sections; never hold a lock across await.
//!
//!
//! Send/receive cipher states are split into [`SendCipher`] / [`RecvCipher`];
//! send and receive paths lock independently with no contention: the send counter belongs to
//! `SendCipher` and the receive counter to `RecvCipher`.

use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes256;
use base64::prelude::{BASE64_STANDARD, BASE64_STANDARD_NO_PAD};
use base64::Engine;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p384::ecdh::diffie_hellman;
use p384::pkcs8::{DecodePublicKey, EncodePrivateKey, EncodePublicKey};
use p384::{PublicKey, SecretKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum EncryptionError {
    /// Invalid key length (must be 32 bytes).
    InvalidKeyLength,
}

impl Display for EncryptionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EncryptionError::InvalidKeyLength => write!(f, "invalid key length"),
        }
    }
}

impl std::error::Error for EncryptionError {}

pub struct MinecraftEncryption;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    salt: String,
}

/// ECDH key exchange and JWT handshake on the P-384 (secp384r1) curve.
impl MinecraftEncryption {
    pub const MOJANG_PUBLIC_KEY: &'static str = "MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAECRXueJeTDqNRRgJi/vlRufByu/2G0i2Ebt6YMar5QX/R0DIIyrJMcUpruK4QveTfJSTp3Shlq4Gk34cD/4GUWwkv0DVuzeuB+tXija7HBxii03NHDbPAD0AKnLr2wdAp";

    pub fn create_token() -> [u8; 16] {
        rand::random()
    }

    /// Generate a P-384 key pair.
    pub fn get_key_pair() -> Option<(SecretKey, PublicKey)> {
        let secret = SecretKey::random(&mut OsRng);
        let public = secret.public_key();
        Some((secret, public))
    }

    /// Parse the base64 (DER SPKI) public key from the client Login packet.
    pub fn parse_key(key: &str) -> Option<PublicKey> {
        let der = BASE64_STANDARD_NO_PAD.decode(key).ok()?;
        PublicKey::from_public_key_der(der.as_slice()).ok()
    }

    pub fn get_shared_secret(
        local_private_key: &SecretKey,
        remote_public_key: &PublicKey,
    ) -> Option<Vec<u8>> {
        let shared_secret = diffie_hellman(
            local_private_key.to_nonzero_scalar(),
            remote_public_key.as_affine(),
        );
        Some(shared_secret.raw_secret_bytes().to_vec())
    }

    pub fn get_secret_key(
        local_private_key: &SecretKey,
        remote_public_key: &PublicKey,
        token: [u8; 16],
    ) -> Option<Vec<u8>> {
        let shared_secret = Self::get_shared_secret(local_private_key, remote_public_key)?;
        let mut hasher = Sha256::new();
        hasher.update(token.as_slice());
        hasher.update(shared_secret.as_slice());
        Some(hasher.finalize().to_vec())
    }

    pub fn create_handshake_jwt(
        key_pair: &(SecretKey, PublicKey),
        token: [u8; 16],
    ) -> Option<String> {
        let mut header = Header::new(Algorithm::ES384);
        // x5u: base64 of the X.509 SubjectPublicKeyInfo DER.
        let public_der = key_pair.1.to_public_key_der().ok()?;
        header.x5u = Some(BASE64_STANDARD.encode(public_der.as_bytes()));
        header.typ = None;
        let claims = Claims {
            salt: BASE64_STANDARD.encode(token),
        };
        let pkcs8 = key_pair.0.to_pkcs8_der().ok()?;
        let key = EncodingKey::from_ec_der(pkcs8.as_bytes());
        encode(&header, &claims, &key).ok()
    }
}

/// AES-256-GCM streaming key (protocol >= 428).
///
/// Bedrock "GCM" is a CTR keystream (no tag): J0 = iv12||be32(1);
/// the data keystream starts at inc32(J0) = iv12||be32(2) with only the low 32 bits incrementing
/// (GCM inc32 semantics).
struct GcmCtrStream {
    aes: Aes256,
    /// [iv12 || be32(counter)]
    counter_block: [u8; 16],
    counter: u32,
    keystream: [u8; 16],
    /// Consumed keystream bytes (generate the next block at 16).
    pos: usize,
}

impl GcmCtrStream {
    fn new(key: &[u8; 32], iv12: &[u8; 12]) -> Self {
        let mut counter_block = [0u8; 16];
        counter_block[..12].copy_from_slice(iv12);
        counter_block[12..].copy_from_slice(&2u32.to_be_bytes());
        Self {
            aes: Aes256::new(key.into()),
            counter_block,
            counter: 2,
            keystream: [0u8; 16],
            pos: 16,
        }
    }

    fn xor_in_place(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            if self.pos == 16 {
                self.counter_block[12..].copy_from_slice(&self.counter.to_be_bytes());
                let mut block = aes::cipher::generic_array::GenericArray::from(self.counter_block);
                self.aes.encrypt_block(&mut block);
                self.keystream = block.into();
                self.pos = 0;
                // inc32: only the low 32 bits increment (2^32 blocks = 64 GiB, unreachable per connection).
                self.counter = self.counter.wrapping_add(1);
            }
            *byte ^= self.keystream[self.pos];
            self.pos += 1;
        }
    }
}

/// AES-256-CFB8 stream (protocol < 428).
///
/// Byte-wise feedback: `ks = E(shift)[0]`, `ct = pt ^ ks`; the feedback register
/// shifts in the ciphertext byte. Encryption and decryption are separate functions:
/// feedback always shifts in the ciphertext byte, which is the output when encrypting and the input when decrypting.
struct Cfb8Stream {
    aes: Aes256,
    /// Feedback register (initial 16-byte IV).
    shift: [u8; 16],
}

impl Cfb8Stream {
    fn new(key: &[u8; 32], iv16: &[u8; 16]) -> Self {
        Self {
            aes: Aes256::new(key.into()),
            shift: *iv16,
        }
    }

    fn encrypt_in_place(&mut self, data: &mut [u8]) {
        let mut block = aes::cipher::generic_array::GenericArray::from(self.shift);
        for byte in data.iter_mut() {
            self.aes.encrypt_block(&mut block);
            let ct = *byte ^ block[0];
            *byte = ct;
            block.copy_within(1..16, 0);
            block[15] = ct;
        }
        self.shift = block.into();
    }

    fn decrypt_in_place(&mut self, data: &mut [u8]) {
        let mut block = aes::cipher::generic_array::GenericArray::from(self.shift);
        for byte in data.iter_mut() {
            self.aes.encrypt_block(&mut block);
            let ct = *byte;
            *byte = ct ^ block[0];
            block.copy_within(1..16, 0);
            block[15] = ct;
        }
        self.shift = block.into();
    }
}

enum StreamKind {
    GcmCtr(GcmCtrStream),
    Cfb8(Cfb8Stream),
}

impl StreamKind {
    fn new(secret_key: &[u8], protocol_version: u32) -> Result<Self, EncryptionError> {
        let key: &[u8; 32] = secret_key
            .get(..32)
            .and_then(|k| k.try_into().ok())
            .ok_or(EncryptionError::InvalidKeyLength)?;
        if protocol_version < 428 {
            // CFB8 uses a 16-byte IV: key[..16].
            let iv: [u8; 16] = key[..16].try_into().unwrap();
            Ok(StreamKind::Cfb8(Cfb8Stream::new(key, &iv)))
        } else {
            // GCM (>= 428) uses a 12-byte IV: key[..12].
            let iv: [u8; 12] = key[..12].try_into().unwrap();
            Ok(StreamKind::GcmCtr(GcmCtrStream::new(key, &iv)))
        }
    }

    fn encrypt_in_place(&mut self, data: &mut [u8]) {
        match self {
            StreamKind::GcmCtr(stream) => stream.xor_in_place(data),
            StreamKind::Cfb8(stream) => stream.encrypt_in_place(data),
        }
    }

    fn decrypt_in_place(&mut self, data: &mut [u8]) {
        match self {
            StreamKind::GcmCtr(stream) => stream.xor_in_place(data),
            StreamKind::Cfb8(stream) => stream.decrypt_in_place(data),
        }
    }
}

fn compute_checksum(bytes: &[u8], count: u32, secret_key: &[u8]) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update((count as i64).to_le_bytes());
    hasher.update(bytes);
    hasher.update(secret_key);
    let output = hasher.finalize();
    let mut checksum = [0u8; 8];
    checksum.copy_from_slice(&output[0..8]);
    checksum
}

/// Outbound encryption state: only touched by the send path.
pub struct SendCipher {
    stream: StreamKind,
    send_count: u32,
    secret_key: Vec<u8>,
}

impl SendCipher {
    pub fn new(secret_key: Vec<u8>, protocol_version: u32) -> Result<Self, EncryptionError> {
        let stream = StreamKind::new(&secret_key, protocol_version)?;
        Ok(Self {
            stream,
            send_count: 0,
            secret_key,
        })
    }

    /// Append an 8-byte checksum then encrypt the whole segment (synchronous, call inside the lock).
    pub fn encode(&mut self, bytes: &[u8]) -> Vec<u8> {
        let checksum = compute_checksum(bytes, self.send_count, &self.secret_key);
        self.send_count = self.send_count.wrapping_add(1);

        let mut output = bytes.to_vec();
        output.extend_from_slice(&checksum);
        self.stream.encrypt_in_place(&mut output);
        output
    }
}

/// Inbound decryption state: only touched by the receive path.
pub struct RecvCipher {
    stream: StreamKind,
    receive_count: u32,
    secret_key: Vec<u8>,
}

impl RecvCipher {
    pub fn new(secret_key: Vec<u8>, protocol_version: u32) -> Result<Self, EncryptionError> {
        let stream = StreamKind::new(&secret_key, protocol_version)?;
        Ok(Self {
            stream,
            receive_count: 0,
            secret_key,
        })
    }

    /// Decrypt and verify the trailing checksum (synchronous, call inside the lock).
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Vec<u8>, std::io::Error> {
        // Reject frames shorter than the 8-byte checksum instead of underflowing.
        if bytes.len() < 8 {
            return Err(std::io::Error::other("encrypted frame too short"));
        }

        let mut packet = bytes.to_vec();
        self.stream.decrypt_in_place(&mut packet);

        let output = packet[..packet.len() - 8].to_vec();
        let output_checksum: [u8; 8] = packet[packet.len() - 8..].try_into().unwrap();
        let computed_checksum = compute_checksum(&output, self.receive_count, &self.secret_key);
        if output_checksum != computed_checksum {
            return Err(std::io::Error::other("checksum error"));
        }
        self.receive_count = self.receive_count.wrapping_add(1);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> Vec<u8> {
        (0u8..32).collect()
    }

    #[test]
    fn gcm_stream_round_trip_matches_counter_semantics() {
        let mut send = SendCipher::new(secret(), 685).unwrap();
        let mut recv = RecvCipher::new(secret(), 685).unwrap();

        let payload = b"hello bedrock encryption".to_vec();
        let encrypted = send.encode(&payload);
        assert_ne!(&encrypted[..], payload.as_slice());
        let decrypted = recv.decode(&encrypted).unwrap();
        assert_eq!(decrypted, payload);

        // Consecutive frames: the counter increments with no cross-frame interference.
        let second = send.encode(b"second frame");
        assert_eq!(recv.decode(&second).unwrap(), b"second frame");
    }

    #[test]
    fn cfb8_stream_round_trip() {
        let mut send = SendCipher::new(secret(), 400).unwrap();
        let mut recv = RecvCipher::new(secret(), 400).unwrap();

        let payload = b"legacy cfb8 frame".to_vec();
        let encrypted = send.encode(&payload);
        let decrypted = recv.decode(&encrypted).unwrap();
        assert_eq!(decrypted, payload);
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let mut send = SendCipher::new(secret(), 685).unwrap();
        let mut recv = RecvCipher::new(secret(), 685).unwrap();

        let mut encrypted = send.encode(b"corrupt me");
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xff;
        assert!(recv.decode(&encrypted).is_err());
    }

    #[test]
    fn invalid_key_length_is_rejected() {
        assert!(SendCipher::new(vec![0u8; 31], 685).is_err());
        assert!(RecvCipher::new(vec![0u8; 31], 685).is_err());
    }
}
