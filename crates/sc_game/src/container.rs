//! The container window a player currently has open (`ContainerOpen`).
//!
//! # Single source of truth
//!
//! One client holds at most one container window, so the open window must be
//! a single per-player fact, never split across fields.
//!
//! Component presence means "window open"; removing it closes the window,
//! so close cannot fail or leak a stuck marker.
//!
//! # Window ids
//!
//! `window_id` is server-assigned per player, sent with `ContainerOpen` and
//! echoed back on close (see [`crate::crafting::ContainerWindowAllocator`]).
//! It is not a global allocator: two players may hold window 1 at once.
//!
//! The client's echoed `container_type` is untrusted, so close always uses
//! this component's recorded `window_id`.

use sc_ecs::component::Component;
use sc_recipe::StationKind;
use sc_world::manager::MinecraftWorldId;

/// Player inventory window id (always 0).
pub const PLAYER_INVENTORY_WINDOW_ID: u8 = 0;

/// The container kind a player currently has open.
/// Variants split by what the container is backed by, not by protocol bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    /// The player's own inventory (no block position or world binding).
    PlayerInventory,
    /// Block workstation (crafting table, stonecutter, ...).
    ///
    /// World and position revalidate the window when craft requests arrive:
    /// the player must still stand before the same block.
    Workstation {
        station: StationKind,
        world_id: MinecraftWorldId,
        position: (i32, i32, i32),
    },
}

/// The container window a player currently has open.
/// Component presence means open; removal means closed.
#[derive(Component, Clone, Debug)]
pub struct ContainerOpen {
    /// Server-assigned window id (inventory is always zero).
    pub window_id: u8,
    pub kind: ContainerKind,
}

impl ContainerOpen {
    /// Open the player inventory.
    pub fn player_inventory() -> Self {
        Self {
            window_id: PLAYER_INVENTORY_WINDOW_ID,
            kind: ContainerKind::PlayerInventory,
        }
    }

    /// Open a block workstation window.
    pub fn at_workstation(
        window_id: u8,
        station: StationKind,
        world_id: MinecraftWorldId,
        position: (i32, i32, i32),
    ) -> Self {
        Self {
            window_id,
            kind: ContainerKind::Workstation {
                station,
                world_id,
                position,
            },
        }
    }

    /// Type name for diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match &self.kind {
            ContainerKind::PlayerInventory => "player_inventory",
            ContainerKind::Workstation { station, .. } => station.tag(),
        }
    }

    /// Whether this is the player inventory window.
    pub fn is_player_inventory(&self) -> bool {
        matches!(self.kind, ContainerKind::PlayerInventory)
    }

    /// Protocol `ContainerType` byte for the registered container kind.
    ///
    /// Both open and close replies must use it, never the client-echoed
    /// type byte: a wrong close type corrupts client window tracking and
    /// later windows close instantly.
    pub fn protocol_type(&self) -> i8 {
        match self.kind {
            ContainerKind::PlayerInventory => -1, // INVENTORY
            ContainerKind::Workstation { station, .. } => match station {
                StationKind::CraftingTable => 1, // WORKBENCH
                StationKind::Stonecutter => 29,  // STONECUTTER
                // Unmapped workstations fall back to CONTAINER(0), never panic.
                _ => 0,
            },
        }
    }

    /// Workstation details `(type, world, block position)`; `None` for inventory.
    ///
    /// Returned by value so callers can `remove_component` first and keep reading.
    pub fn workstation(&self) -> Option<(StationKind, MinecraftWorldId, (i32, i32, i32))> {
        match &self.kind {
            ContainerKind::PlayerInventory => None,
            ContainerKind::Workstation {
                station,
                world_id,
                position,
            } => Some((*station, world_id.clone(), *position)),
        }
    }

    /// Whether the window is bound to a `station` workstation.
    pub fn is_station(&self, station: StationKind) -> bool {
        matches!(
            self.kind,
            ContainerKind::Workstation {
                station: current,
                ..
            } if current == station
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_inventory_window_has_window_id_zero_and_no_station() {
        let open = ContainerOpen::player_inventory();
        assert_eq!(open.window_id, 0);
        assert!(open.is_player_inventory());
        assert!(open.workstation().is_none());
    }

    #[test]
    fn workstation_window_carries_station_world_and_position() {
        let world_id = MinecraftWorldId::random();
        let open = ContainerOpen::at_workstation(3, StationKind::CraftingTable, world_id.clone(), (1, 2, 3));
        assert_eq!(open.window_id, 3);
        assert!(!open.is_player_inventory());
        assert!(open.is_station(StationKind::CraftingTable));
        assert!(!open.is_station(StationKind::Stonecutter));
        let (station, id, position) = open.workstation().expect("workstation");
        assert_eq!(station, StationKind::CraftingTable);
        assert_eq!(id, world_id);
        assert_eq!(position, (1, 2, 3));
    }

    #[test]
    fn protocol_type_uses_server_window_type_not_client_bytes() {
        // Close replies echo the registered window type, never client bytes.
        assert_eq!(ContainerOpen::player_inventory().protocol_type(), -1);
        let world_id = MinecraftWorldId::random();
        assert_eq!(
            ContainerOpen::at_workstation(1, StationKind::CraftingTable, world_id.clone(), (0, 0, 0))
                .protocol_type(),
            1
        );
        assert_eq!(
            ContainerOpen::at_workstation(2, StationKind::Stonecutter, world_id, (0, 0, 0))
                .protocol_type(),
            29
        );
    }
}