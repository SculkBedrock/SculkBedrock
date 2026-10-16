//! Creative-mode grant table: creative listing ids to grantable stack templates.

use crate::stack::ItemStack;
use sc_ecs::resource::Resource;

/// Creative-mode grant table: dense 1-based creative entry ids (exactly the
/// ids sent in this server's CreativeContent listing) to grantable stack
/// templates (count is always 1; the granted amount comes from the
/// request's own Output-consuming Moves at execution).
///
/// Entries whose creative payload carries NBT grant their base item:
/// [`ItemStack`] carries no NBT, so NBT extras (potions, enchanted books)
/// do not round-trip through creative takes yet.
#[derive(Resource, Clone, Debug, Default)]
pub struct CreativeGrantTable {
    templates: Vec<ItemStack>,
}

impl CreativeGrantTable {
    /// Pushes one entry, returning its 1-based creative id.
    pub fn push(&mut self, template: ItemStack) -> u32 {
        self.templates.push(template);
        self.templates.len() as u32
    }

    /// Template for a creative id sent by the client, if listed.
    pub fn get(&self, creative_id: u32) -> Option<ItemStack> {
        creative_id
            .checked_sub(1)
            .and_then(|index| self.templates.get(index as usize))
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}
