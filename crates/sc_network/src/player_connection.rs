//! `PlayerConnection`: player network connection component.
//!
//! Wraps the RakNet `Connection` plus encryption/compression and protocol
//! codec; `send_packet`/`send_batch` are the send entries and the state
//! machine (Logging -> Initializing -> InGame) drives the login flow.
//! Received packets dispatch to handlers via `MinecraftPacketReceiver<T>`.

use crate::events::connection::DropConnection;
use crate::packet::batch_packet::BatchPacket;
use crate::packet::decoder::PackerDecoder;
use crate::packet::encoder::PackerEncoder;
use crate::packet::raw_batch::RawBatchPacket;
use crate::packet::PacketEncryptionError;
use crate::packet_hooks::PacketSendHooks;
use crate::protocol::client::login::{Login, LoginDevice};
use crate::protocol::server::game::Disconnect;
use crate::protocol::{MinecraftPacket, MinecraftPackets};
use crate::utils::compression_algorithm::CompressionAlgorithm;
use crate::utils::encryption::{EncryptionError, RecvCipher, SendCipher};
use parking_lot::RwLock;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_eventbus::SCSendEvent;
use sc_raknet::connection::queue::SendQueueError;
use sc_raknet::connection::{Connection, RecvError};
use sc_utils::game::skin::Skin;
use sc_utils::world::client_data::MinecraftClientData;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::Display;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;
use uuid::Uuid;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PlayerConnectionStatus {
    None,
    Logging,
    Handshaking,
    ResourcePack,
    PreSpawn,
    Initializing,
    AwaitingClientInitialization,
    InGame,
    Spawned,
}

/// Result after a packet passes hooks and is offered to the connection queue.
/// `Queued` is not a UDP-send or client-acknowledgement guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketSendOutcome {
    Queued,
    CancelledByHook,
    SuppressedByDebugFilter,
    StaleContext,
    StaleContent,
    BudgetDeferred,
}

#[derive(Debug)]
pub enum TrySendPacketError {
    Busy,
    Connection(PlayerConnectionError),
}

impl Display for TrySendPacketError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("outbound command slots busy"),
            Self::Connection(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for TrySendPacketError {}

impl PlayerConnectionStatus {
    /// Chunk phase: the client has finished the before-spawn registry exchange
    /// and may request chunk radius / receive chunks.
    pub fn accepts_chunk_radius(self) -> bool {
        matches!(
            self,
            PlayerConnectionStatus::Initializing
                | PlayerConnectionStatus::AwaitingClientInitialization
                | PlayerConnectionStatus::InGame
                | PlayerConnectionStatus::Spawned
        )
    }

    /// Chunk phase: once the client has requested a chunk radius, the server
    /// may stream chunks while waiting for the initial PLAYER_SPAWN /
    /// local-player initialization handshake to complete.
    pub fn can_send_chunks(self) -> bool {
        self.accepts_chunk_radius()
    }

    pub fn accepts_local_player_initialized(self) -> bool {
        matches!(
            self,
            PlayerConnectionStatus::AwaitingClientInitialization
                | PlayerConnectionStatus::InGame
                | PlayerConnectionStatus::Spawned
        )
    }
}

#[derive(Debug)]
pub enum PlayerConnectionError {
    EncryptionInitError(EncryptionError),
    RecvError(RecvError),
    EncryptionError(PacketEncryptionError),
    SendQueueError(SendQueueError),
    PacketNotQueued(PacketSendOutcome),
    /// A hard context change (teleport / dimension / world) landed while this
    /// packet was being prepared. The packet is dropped **before** a cipher
    /// counter is consumed, so no sequence gap is created (§11.4 point 2).
    StaleContext,
}

impl Display for PlayerConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for PlayerConnectionError {}

#[derive(Component, Clone)]
pub struct PlayerConnection {
    pub(crate) connection: Connection,
    world: World,
    entity: EntityId,
    /// Outbound encryption state: only touched by the send path (independent
    /// from recv_cipher, so the two paths share no lock).
    send_cipher: Arc<RwLock<Option<SendCipher>>>,
    outbound_failed: Arc<AtomicBool>,
    outbound_cleanup_scheduled: Arc<AtomicBool>,
    /// Hard delivery-context generation (§11.4).
    delivery_barrier: Arc<DeliveryBarrier>,
    /// Inbound decryption state: only touched by the receive path.
    recv_cipher: Arc<RwLock<Option<RecvCipher>>>,
    encoder: Arc<RwLock<PackerEncoder>>,
    decoder: Arc<RwLock<PackerDecoder>>,
    protocol_version: Arc<AtomicU32>,
    chunk_radius: Arc<AtomicU32>,
    status: Arc<RwLock<PlayerConnectionStatus>>,
    data: Arc<RwLock<PlayerConnectionData>>,
    resource_chunks_requested: Arc<RwLock<HashMap<Uuid, HashSet<u32>>>>,
    pub(crate) resource_pack_chunk_semaphore: Arc<Semaphore>,
}

/// Hard delivery-context generation for one connection (§11.4).
///
/// A teleport, dimension change or world change advances the generation. Any
/// packet that started preparing under an older generation is refused *before*
/// it consumes an encryption counter or a reliable sequence number, while
/// packets that were already admitted keep their place in the ordered send
/// queue. That is what makes a context switch an ordered barrier rather than a
/// best-effort epoch check.
#[derive(Debug)]
pub struct DeliveryBarrier {
    generation: AtomicU64,
}

impl DeliveryBarrier {
    pub fn new() -> Self {
        Self {
            generation: AtomicU64::new(1),
        }
    }

    pub fn current(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Advance to the next context and return its generation.
    pub fn begin(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn is_current(&self, barrier: u64) -> bool {
        self.generation.load(Ordering::Acquire) == barrier
    }
}

impl Default for DeliveryBarrier {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerConnection {
    pub fn new(connection: Connection, world: World, entity: EntityId) -> Self {
        Self {
            connection,
            world,
            entity,
            send_cipher: Arc::new(RwLock::new(None)),
            outbound_failed: Arc::new(AtomicBool::new(false)),
            outbound_cleanup_scheduled: Arc::new(AtomicBool::new(false)),
            delivery_barrier: Arc::new(DeliveryBarrier::new()),
            recv_cipher: Arc::new(RwLock::new(None)),
            encoder: Arc::new(RwLock::new(PackerEncoder::new())),
            decoder: Arc::new(RwLock::new(PackerDecoder::new())),
            protocol_version: Arc::new(AtomicU32::new(0)),
            chunk_radius: Arc::new(AtomicU32::new(0)),
            status: Arc::new(RwLock::new(PlayerConnectionStatus::None)),
            data: Arc::new(RwLock::new(PlayerConnectionData::new())),
            resource_chunks_requested: Arc::new(RwLock::new(HashMap::new())),
            resource_pack_chunk_semaphore: Arc::new(Semaphore::new(1)),
        }
    }

    /// Current hard delivery-context generation.
    pub fn context_barrier(&self) -> u64 {
        self.delivery_barrier.current()
    }

    /// Start a new hard delivery context and return its generation.
    ///
    /// §11.4: a teleport / dimension / world switch must be an ordered barrier on
    /// this connection. Control packets of the new context are produced after
    /// this call and therefore queue behind everything the old context already
    /// admitted; in-flight preparations from the old context are refused instead
    /// of consuming a cipher counter.
    pub fn begin_context_barrier(&self) -> u64 {
        self.delivery_barrier.begin()
    }

    /// Whether a preparation started under `barrier` may still be admitted.
    pub fn barrier_is_current(&self, barrier: u64) -> bool {
        self.delivery_barrier.is_current(barrier)
    }

    pub fn enable_compression(&self, compression: CompressionAlgorithm) {
        self.decoder
            .write()
            .set_compression_algorithm(Some(compression));
        self.encoder
            .write()
            .set_compression_algorithm(Some(compression));
    }

    pub fn disable_compression(&self) {
        self.decoder.write().set_compression_algorithm(None);
        self.encoder.write().set_compression_algorithm(None);
    }

    pub fn enable_encryption(&self, secret_key: Vec<u8>) -> Result<(), PlayerConnectionError> {
        // Build send/recv cipher states independently: the send counter belongs
        // to SendCipher and the receive counter to RecvCipher.
        let send = SendCipher::new(secret_key.clone(), self.get_protocol_version())
            .map_err(PlayerConnectionError::EncryptionInitError)?;
        let recv = RecvCipher::new(secret_key, self.get_protocol_version())
            .map_err(PlayerConnectionError::EncryptionInitError)?;
        *self.send_cipher.write() = Some(send);
        *self.recv_cipher.write() = Some(recv);
        Ok(())
    }

    pub async fn recv(&self) -> Result<BatchPacket, PlayerConnectionError> {
        let packet_data = self
            .connection
            .recv()
            .await
            .map_err(|e| PlayerConnectionError::RecvError(e))?;
        // Decrypt + decompress + parse is a synchronous CPU critical section:
        // the lock is released at the end of the block and never held across await.
        let batch = {
            let mut cipher = self.recv_cipher.write();
            crate::protocol::version::with_protocol_version(self.get_protocol_version(), || {
                self.decoder
                    .read()
                    .decode(packet_data, cipher.as_mut())
                    .map_err(|e| PlayerConnectionError::EncryptionError(e))
            })?
        };
        crate::packet::dump::record_batch("in", Some(self.entity), "minecraft", &batch);
        Ok(batch)
    }

    pub async fn send_packet<P: MinecraftPacket + Clone + 'static + Send + Sync>(
        &self,
        packet: P,
        immediate: bool,
    ) -> Result<(), PlayerConnectionError> {
        self.send_packet_with_outcome(packet, immediate)
            .await
            .map(|_| ())
    }

    /// Send a packet and distinguish queue admission from intentional suppression.
    /// This allows stateful pipelines to avoid treating a hook-cancelled or
    /// debug-filtered packet as delivered while preserving `send_packet`'s API.
    pub async fn send_packet_with_outcome<P: MinecraftPacket + Clone + 'static + Send + Sync>(
        &self,
        packet: P,
        immediate: bool,
    ) -> Result<PacketSendOutcome, PlayerConnectionError> {
        // Derive-generated type lookup gets the discriminant without converting
        // or cloning the packet. Keep the clone fallback for custom packet
        // implementations that are not a direct MinecraftPackets variant.
        let packet_id = MinecraftPackets::packet_id_for_type::<P>()
            .unwrap_or_else(|| packet.clone().to_packets().id());

        if self.debug_packet_blocked(packet_id) {
            let name = crate::packet::dump::packet_variant_name(&packet.clone().to_packets());
            log::debug!(
                "[packet-filter] dropped {name} (0x{packet_id:x}) per debug.blocked_packets"
            );
            return Ok(PacketSendOutcome::SuppressedByDebugFilter);
        }
        let barrier = self.context_barrier();
        let permit = self
            .connection
            .reserve_outbound_slot()
            .await
            .map_err(PlayerConnectionError::SendQueueError)?;

        self.send_packet_with_reserved_outcome(packet, immediate, packet_id, permit, barrier)
            .await
    }

    pub(crate) async fn send_packet_with_checked_outcome<P, A>(
        &self,
        packet: P,
        immediate: bool,
        admit: A,
    ) -> Result<PacketSendOutcome, PlayerConnectionError>
    where
        P: MinecraftPacket + Clone + 'static + Send + Sync,
        A: FnOnce(
            Vec<u8>,
            sc_raknet::connection::OutboundSlotPermit,
            &mut Option<Vec<u8>>,
        ) -> Result<PacketSendOutcome, PlayerConnectionError>,
    {
        let packet_id = MinecraftPackets::packet_id_for_type::<P>()
            .unwrap_or_else(|| packet.clone().to_packets().id());
        if self.debug_packet_blocked(packet_id) {
            return Ok(PacketSendOutcome::SuppressedByDebugFilter);
        }
        let barrier = self.context_barrier();
        let permit = self
            .connection
            .reserve_outbound_slot()
            .await
            .map_err(PlayerConnectionError::SendQueueError)?;
        self.prepare_packet_and_admit(packet, immediate, packet_id, permit, barrier, admit)
            .await
    }

    /// Try final packet admission before constructing a large packet payload,
    /// running hooks, or encoding. Busy is an explicit retryable result.
    pub async fn try_send_packet_with_outcome<P, F, G>(
        &self,
        packet_factory: F,
        immediate: bool,
        queue_guard: G,
    ) -> Result<PacketSendOutcome, TrySendPacketError>
    where
        P: MinecraftPacket + Clone + 'static + Send + Sync,
        F: FnOnce() -> P,
        G: Send + 'static,
    {
        self.try_send_packet_with_checked_outcome(
            packet_factory,
            immediate,
            queue_guard,
            |plain, permit, trace| {
                self.admit_prepared(plain, immediate, permit, trace)?;
                Ok(PacketSendOutcome::Queued)
            },
        )
        .await
    }

    pub(crate) async fn try_send_packet_with_checked_outcome<P, F, G, A>(
        &self,
        packet_factory: F,
        immediate: bool,
        queue_guard: G,
        admit: A,
    ) -> Result<PacketSendOutcome, TrySendPacketError>
    where
        P: MinecraftPacket + Clone + 'static + Send + Sync,
        F: FnOnce() -> P,
        G: Send + 'static,
        A: FnOnce(
            Vec<u8>,
            sc_raknet::connection::OutboundSlotPermit,
            &mut Option<Vec<u8>>,
        ) -> Result<PacketSendOutcome, PlayerConnectionError>,
    {
        let known_packet_id = MinecraftPackets::packet_id_for_type::<P>();
        if let Some(packet_id) = known_packet_id {
            if self.debug_packet_blocked(packet_id) {
                log::debug!(
                    "[packet-filter] dropped {} (0x{packet_id:x}) per debug.blocked_packets",
                    std::any::type_name::<P>()
                );
                return Ok(PacketSendOutcome::SuppressedByDebugFilter);
            }
        }

        let barrier = self.context_barrier();
        let mut permit =
            self.connection
                .try_reserve_outbound_slot()
                .map_err(|error| match error {
                    sc_raknet::connection::OutboundAdmissionError::Busy => TrySendPacketError::Busy,
                    sc_raknet::connection::OutboundAdmissionError::Closed => {
                        TrySendPacketError::Connection(PlayerConnectionError::SendQueueError(
                            SendQueueError::SendError,
                        ))
                    }
                })?;
        permit.attach_guard(queue_guard);
        let packet = packet_factory();
        let packet_id = known_packet_id.unwrap_or_else(|| packet.clone().to_packets().id());
        self.prepare_packet_and_admit(packet, immediate, packet_id, permit, barrier, admit)
            .await
            .map_err(TrySendPacketError::Connection)
    }

    async fn send_packet_with_reserved_outcome<
        P: MinecraftPacket + Clone + 'static + Send + Sync,
    >(
        &self,
        packet: P,
        immediate: bool,
        packet_id: u16,
        permit: sc_raknet::connection::OutboundSlotPermit,
        barrier: u64,
    ) -> Result<PacketSendOutcome, PlayerConnectionError> {
        self.prepare_packet_and_admit(
            packet,
            immediate,
            packet_id,
            permit,
            barrier,
            |plain, permit, trace| {
                self.admit_prepared(plain, immediate, permit, trace)?;
                Ok(PacketSendOutcome::Queued)
            },
        )
        .await
    }

    async fn prepare_packet_and_admit<P, A>(
        &self,
        packet: P,
        immediate: bool,
        packet_id: u16,
        permit: sc_raknet::connection::OutboundSlotPermit,
        barrier: u64,
        admit: A,
    ) -> Result<PacketSendOutcome, PlayerConnectionError>
    where
        P: MinecraftPacket + Clone + 'static + Send + Sync,
        A: FnOnce(
            Vec<u8>,
            sc_raknet::connection::OutboundSlotPermit,
            &mut Option<Vec<u8>>,
        ) -> Result<PacketSendOutcome, PlayerConnectionError>,
    {
        // Debug filter ([debug] blocked_packets): `send_packet` is the single
        // outbound entry, so intercept by packet id here without encoding or
        // RakNet admission. Takes effect after a restart, no recompile needed.
        if self.debug_packet_blocked(packet_id) {
            // This path suppresses the send, so the rare debug-only clone is
            // preferable to imposing a payload clone on every normal send.
            let name = crate::packet::dump::packet_variant_name(&packet.clone().to_packets());
            log::debug!(
                "[packet-filter] dropped {name} (0x{packet_id:x}) per debug.blocked_packets"
            );
            return Ok(PacketSendOutcome::SuppressedByDebugFilter);
        }
        // ⓪ Barrier check before any plugin-visible work: a hard context change
        // that already happened means this packet belongs to a retired context.
        if !self.barrier_is_current(barrier) {
            return Ok(PacketSendOutcome::StaleContext);
        }
        // 1. Sync dispatch: a hook calling set_cancelled(true) cancels this send.
        let hooks = self
            .world
            .get_resource::<PacketSendHooks>()
            .map(|hooks| hooks.snapshot())
            .unwrap_or_default();
        if !hooks.is_empty() {
            let cancelled = PacketSendHooks::dispatch_snapshot(
                hooks,
                self.entity,
                packet_id,
                std::any::type_name::<P>(),
                immediate,
                &packet,
            )
            .await;
            if cancelled {
                return Ok(PacketSendOutcome::CancelledByHook); // Cancelled: skip encoding and sending.
            }
        }
        let batch = BatchPacket::single(packet);
        let delay_ms = debug_send_delay_ms();
        let _serialize_guard = if delay_ms > 0 {
            let guard = debug_send_lock().lock().await;
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            Some(guard)
        } else {
            None
        };
        // ⓑ Re-check after hooks and compression: hooks may await for several
        // ticks, during which a teleport can retire this context. Refusing here
        // keeps the plaintext envelope and, crucially, the cipher counter unused.
        if !self.barrier_is_current(barrier) {
            return Ok(PacketSendOutcome::StaleContext);
        }
        crate::packet::dump::record_batch("out", Some(self.entity), "minecraft", &batch);
        let plain =
            crate::protocol::version::with_protocol_version(self.get_protocol_version(), || {
                self.encoder
                    .read()
                    .prepare(batch)
                    .map_err(PlayerConnectionError::EncryptionError)
            })?;
        let mut trace = None;
        let result = admit(plain, permit, &mut trace);
        self.close_on_admission_failure(&result);
        if let Some(bytes) = trace {
            crate::packet::dump::record_wire(
                "out",
                Some(self.entity),
                "bedrock_wire",
                &bytes,
                "batch",
            );
        }
        let outcome = result?;
        if outcome != PacketSendOutcome::Queued {
            return Ok(outcome);
        }
        // 2. Async observation (eventbus, next tick; zero cost without subscribers).
        let _ = self.world.send_sc_event(
            self.entity,
            crate::packet_hooks::PacketSendEvent {
                entity: self.entity,
                packet_id,
                packet_name: std::any::type_name::<P>(),
                immediate,
            },
        );
        Ok(PacketSendOutcome::Queued)
    }

    /// The cipher lock also orders enqueue. Compression and hooks have already
    /// completed; no await or diagnostic I/O occurs in this critical section.
    pub(crate) fn admit_prepared(
        &self,
        plain: Vec<u8>,
        immediate: bool,
        permit: sc_raknet::connection::OutboundSlotPermit,
        trace: &mut Option<Vec<u8>>,
    ) -> Result<(), PlayerConnectionError> {
        let mut cipher = self.send_cipher.write();
        if self.outbound_failed.load(Ordering::Acquire) {
            return Err(PlayerConnectionError::SendQueueError(
                SendQueueError::SendError,
            ));
        }
        let bytes = PackerEncoder::finish(plain, cipher.as_mut());
        if crate::packet::dump::enabled() {
            *trace = Some(bytes.clone());
        }
        let result = self
            .connection
            .try_send_owned_with_permit(bytes, immediate, permit);
        if result.is_err() {
            self.outbound_failed.store(true, Ordering::Release);
        }
        result.map_err(PlayerConnectionError::SendQueueError)
    }

    fn close_on_admission_failure<T>(&self, result: &Result<T, PlayerConnectionError>) {
        if matches!(result, Err(PlayerConnectionError::SendQueueError(_)))
            && !self.outbound_cleanup_scheduled.swap(true, Ordering::AcqRel)
        {
            // A consumed cipher counter cannot be retried or skipped. Closing
            // also wakes callers awaiting an outbound count permit.
            let connection = self.connection.clone();
            sc_ecs::async_manager::SCECSAsync::runtime().spawn(async move {
                connection.close().await;
            });
            self.world.send_event(DropConnection {
                entity: self.entity,
            });
        }
    }

    fn debug_packet_blocked(&self, packet_id: u16) -> bool {
        self.world
            .get_resource::<sc_utils::game::structs::server_properties::ServerProperties>()
            .is_some_and(|properties| properties.debug_blocked_packets.contains(&packet_id))
    }

    pub async fn send_batch(
        &self,
        packet: BatchPacket,
        immediate: bool,
    ) -> Result<(), PlayerConnectionError> {
        let barrier = self.context_barrier();
        let permit = self
            .connection
            .reserve_outbound_slot()
            .await
            .map_err(PlayerConnectionError::SendQueueError)?;
        self.send_batch_with_permit(packet, immediate, permit, barrier)
            .await
    }

    async fn send_batch_with_permit(
        &self,
        packet: BatchPacket,
        immediate: bool,
        permit: sc_raknet::connection::OutboundSlotPermit,
        barrier: u64,
    ) -> Result<(), PlayerConnectionError> {
        // Debug: per-packet delay on all paths (SC_SEND_DELAY_MS in ms; 0 = no delay).
        // The login sequence, chunk pipeline, and first spawn all pass through
        // here; the delay plus a mutex serializes sends at a fixed interval, so
        // the last packet before a client crash identifies the crashing packet.
        let delay_ms = debug_send_delay_ms();
        let _serialize_guard = if delay_ms > 0 {
            let guard = debug_send_lock().lock().await;
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            Some(guard)
        } else {
            None
        };
        crate::packet::dump::record_batch("out", Some(self.entity), "minecraft", &packet);
        // Compression + encryption is a synchronous CPU critical section: the
        // lock is released at the end of the block and never held across await.
        let plain =
            crate::protocol::version::with_protocol_version(self.get_protocol_version(), || {
                self.encoder
                    .read()
                    .prepare(packet)
                    .map_err(PlayerConnectionError::EncryptionError)
            })?;
        let mut trace = None;
        let result = self.admit_prepared(plain, immediate, permit, &mut trace);
        self.close_on_admission_failure(&result);
        if let Some(bytes) = trace {
            crate::packet::dump::record_wire(
                "out",
                Some(self.entity),
                "bedrock_wire",
                &bytes,
                "batch",
            );
        }
        result
    }

    /// Send a pre-serialized constant packet (login cache path).
    ///
    /// Differences from [`Self::send_packet`]:
    /// - The body is a cross-player shared `Arc<Vec<u8>>` (`[u16 id BE][payload]`)
    ///   with no per-player rebuild, deep copy, or NBT re-serialization;
    /// - Packet hooks / send events are skipped (contents are constant), while
    ///   the `debug.blocked_packets` filter still applies;
    /// - Compression + encryption still use the per-connection encoder.
    pub async fn send_raw_packet(
        &self,
        packet: Arc<Vec<u8>>,
        immediate: bool,
    ) -> Result<(), PlayerConnectionError> {
        let packet_bytes = packet.as_slice();
        if packet_bytes.len() < 2 {
            return Err(PlayerConnectionError::EncryptionError(
                PacketEncryptionError::IoError(std::io::Error::other(
                    "raw packet missing u16 packet id",
                )),
            ));
        }
        let packet_id = u16::from_be_bytes([packet_bytes[0], packet_bytes[1]]);
        let debug_blocked = self
            .world
            .get_resource::<sc_utils::game::structs::server_properties::ServerProperties>()
            .is_some_and(|properties| properties.debug_blocked_packets.contains(&packet_id));
        if debug_blocked {
            log::debug!(
                "[packet-filter] dropped cached packet 0x{packet_id:x} per debug.blocked_packets"
            );
            return Ok(());
        }
        let permit = self
            .connection
            .reserve_outbound_slot()
            .await
            .map_err(PlayerConnectionError::SendQueueError)?;
        let plain = self
            .encoder
            .read()
            .prepare(RawBatchPacket::single(packet))
            .map_err(PlayerConnectionError::EncryptionError)?;
        let mut trace = None;
        let result = self.admit_prepared(plain, immediate, permit, &mut trace);
        self.close_on_admission_failure(&result);
        if let Some(bytes) = trace {
            crate::packet::dump::record_wire(
                "out",
                Some(self.entity),
                "bedrock_wire",
                &bytes,
                "batch",
            );
        }
        result
    }

    pub async fn disconnect(
        &self,
        reason: &str,
        hide_disconnect_screen: bool,
    ) -> Result<(), PlayerConnectionError> {
        self.disable_compression();
        // Failed sends (dead connection, e.g. client already disconnected) still
        // go through close + event cleanup, otherwise player entities leak.
        let _ = self
            .send_packet(
                Disconnect {
                    hide_disconnect_screen,
                    kick_message: reason.to_string(),
                },
                true,
            )
            .await;
        // Send Disconnect through the RakNet reliable retransmit window before
        // close (about 5s), so the client still receives the kick reason.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        self.connection.close().await;
        self.world.send_event(DropConnection {
            entity: self.entity,
        });
        Ok(())
    }

    pub fn set_protocol_version(&self, protocol_version: u32) {
        self.protocol_version
            .store(protocol_version, Ordering::Relaxed);
    }

    pub fn get_protocol_version(&self) -> u32 {
        self.protocol_version.load(Ordering::Relaxed)
    }

    pub fn set_chunk_radius(&self, radius: i32) {
        self.chunk_radius
            .store(radius.max(0) as u32, Ordering::Relaxed);
    }

    pub fn get_chunk_radius(&self) -> i32 {
        self.chunk_radius.load(Ordering::Relaxed) as i32
    }

    pub fn set_status(&self, status: PlayerConnectionStatus) {
        *self.status.write() = status;
    }

    pub fn get_status(&self) -> PlayerConnectionStatus {
        *self.status.read()
    }

    pub fn set_data(&self, data: PlayerConnectionData) {
        *self.data.write() = data;
    }

    pub fn get_data(&self) -> PlayerConnectionData {
        (*self.data.read()).clone()
    }

    pub fn address(&self) -> SocketAddr {
        self.connection.address
    }

    pub fn get_resource_chunks_requested(&self) -> HashMap<Uuid, HashSet<u32>> {
        (*self.resource_chunks_requested.read()).clone()
    }

    pub fn register_chunk_request(&self, pack_id: Uuid, chunk_index: u32) -> bool {
        let mut map = self.resource_chunks_requested.write();
        map.entry(pack_id)
            .or_insert_with(HashSet::new)
            .insert(chunk_index)
    }

    pub fn clear_chunk_requests(&self) {
        self.resource_chunks_requested.write().clear();
    }

    /// Clears all internal state that could retain significant memory. Called
    /// from `drop_connection` before `world.despawn()` to ensure per-player
    /// buffers are released immediately. The SendQueue/RecvQueue are already
    /// cleared by `Connection::close()`, but the per-player bookkeeping
    /// (resource chunk requests, data) also needs explicit clearing because
    /// these are behind `Arc<RwLock<...>>` and may be referenced by
    /// still-running handler tasks whose futures haven't been dropped yet.
    pub fn cleanup(&self) {
        // Clear resource chunk request tracking — can hold hundreds of entries
        // during a resource pack download.
        let mut chunks = self.resource_chunks_requested.write();
        chunks.clear();
        chunks.shrink_to_fit();
        drop(chunks);

        // Clear player data (skin, username, etc.)
        *self.data.write() = PlayerConnectionData::new();

        // Clear encryption state
        *self.send_cipher.write() = None;
        *self.recv_cipher.write() = None;
    }
}

/// Debug per-packet send delay in ms, read once then cached.
/// Configured via `SC_SEND_DELAY_MS`; missing/invalid means 0 (no delay).
fn debug_send_delay_ms() -> u64 {
    static DELAY_MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *DELAY_MS.get_or_init(|| {
        std::env::var("SC_SEND_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    })
}

/// Global serial lock for the debug delay: keeps send order consistent with
/// call order across concurrent tasks when per-packet delay is enabled.
fn debug_send_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[derive(Clone)]
pub struct PlayerConnectionData {
    pub uuid: Uuid,
    pub xuid: String,
    pub username: String,
    pub client_id: i64,
    pub skin: Option<Skin>,
    pub device_os: LoginDevice,
    pub device_model: String,
    pub language_code: String,
    pub game_version: String,
    pub server_address: String,
}

impl PlayerConnectionData {
    pub fn new() -> Self {
        Self {
            uuid: Uuid::nil(),
            xuid: String::new(),
            username: String::new(),
            client_id: 0,
            skin: None,
            device_os: LoginDevice::Unknown,
            device_model: String::new(),
            language_code: String::new(),
            game_version: String::new(),
            server_address: String::new(),
        }
    }

    pub fn from_login(login: &Login) -> Self {
        Self {
            uuid: login.identity,
            xuid: login.xuid.clone(),
            username: login.username.clone(),
            client_id: login.client_id,
            skin: Some(login.skin.clone()),
            device_os: LoginDevice::from_u8(login.device_os),
            device_model: login.device_model.clone(),
            language_code: login.language_code.clone(),
            game_version: login.game_version.clone(),
            server_address: login.server_address.clone(),
        }
    }

    pub fn to_client_data(&self, protocol_version: u32) -> MinecraftClientData {
        let mut data = MinecraftClientData::default();
        data.uuid = self.uuid;
        data.display_name = self.username.clone();
        data.protocol_version = protocol_version;
        data
    }
}

#[cfg(test)]
mod barrier_tests {
    use super::DeliveryBarrier;

    #[test]
    fn a_hard_context_change_advances_the_generation_and_retires_the_old_one() {
        let barrier = DeliveryBarrier::new();
        let first = barrier.current();
        assert!(barrier.is_current(first));

        let second = barrier.begin();
        assert_eq!(second, first + 1);
        assert!(
            !barrier.is_current(first),
            "a preparation from the retired context must be refused"
        );
        assert!(barrier.is_current(second));
    }

    #[test]
    fn repeated_context_changes_keep_advancing_monotonically() {
        let barrier = DeliveryBarrier::new();
        let mut generation = barrier.current();
        for _ in 0..8 {
            let next = barrier.begin();
            assert_eq!(next, generation + 1);
            generation = next;
        }
        assert!(barrier.is_current(generation));
        assert!(!barrier.is_current(generation - 1));
    }

    #[test]
    fn concurrent_context_changes_still_retire_every_older_generation() {
        use std::sync::Arc;
        let barrier = Arc::new(DeliveryBarrier::new());
        let before = barrier.current();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || barrier.begin())
            })
            .collect();
        let mut seen: Vec<u64> = handles
            .into_iter()
            .map(|handle| handle.join().expect("barrier thread"))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            4,
            "every concurrent switch must get its own generation"
        );
        assert!(seen.iter().all(|generation| *generation > before));
        assert!(!barrier.is_current(before));
    }
}
