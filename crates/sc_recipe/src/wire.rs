//! Protocol-agnostic wire view for `CraftingData`.
//!
//! The network crate owns the actual packet bytes; this module only groups
//! compiled recipes into the Bedrock wire segments so the game side
//! never touches packet types:
//! shaped / shapeless(+furnace) / multi / user / chemistry / smithing
//! transform / smithing trim / brewing / container / material reducer.
//!
//! Only correctly compiled, wire-expressible recipes are listed. Anything
//! else stays in `disabled` with an explicit reason and is never sent.

use crate::compile::{CompiledRecipe, RecipeBody};
use crate::kind::{RecipeKind, StationKind};

/// One wire-expressible recipe with its resolved network id.
#[derive(Clone, Debug)]
pub struct WireRecipe<'a> {
    pub recipe: &'a CompiledRecipe,
    pub network_id: u32,
}

/// Segmented wire view in deterministic network order.
#[derive(Clone, Debug, Default)]
pub struct CraftingWireView<'a> {
    pub shaped: Vec<WireRecipe<'a>>,
    /// Shapeless crafting + furnace entries share the shapeless
    /// segment (type tag inside the entry distinguishes them).
    pub shapeless: Vec<WireRecipe<'a>>,
    pub smithing_transform: Vec<WireRecipe<'a>>,
    pub brewing_mix: Vec<WireRecipe<'a>>,
    pub brewing_container: Vec<WireRecipe<'a>>,
    pub material_reducer: Vec<WireRecipe<'a>>,
    /// (identifier, reason) for recipes that compiled but cannot be
    /// expressed on the 2168 wire.
    pub wire_disabled: Vec<(String, String)>,
}

impl<'a> CraftingWireView<'a> {
    /// Build from a network-ordered recipe slice. `network_id_base` is the
    /// first network id; ids increment in order (trim keeps id 1 reserved
    /// for the hardcoded `minecraft:smithing_armor_trim`, so callers
    /// usually start at 2).
    pub fn build(recipes: &'a [CompiledRecipe], network_id_base: u32) -> Self {
        let mut view = Self::default();
        let mut next = network_id_base.max(1);
        for recipe in recipes {
            let entry = WireRecipe {
                recipe,
                network_id: next,
            };
            match recipe.body {
                RecipeBody::Shaped(_) => {
                    // Only crafting-table shaped recipes go on the wire as
                    // shaped; other stations have no 2168 shaped segment.
                    if recipe.stations.is_empty()
                        || recipe.stations.contains(&StationKind::CraftingTable)
                    {
                        view.shaped.push(entry);
                        next += 1;
                    } else {
                        view.wire_disabled.push((
                            recipe.identifier.clone(),
                            format!(
                                "shaped station {:?} has no 2168 wire segment",
                                recipe.stations
                            ),
                        ));
                    }
                }
                RecipeBody::Shapeless(_) => {
                    view.shapeless.push(entry);
                    next += 1;
                }
                RecipeBody::Furnace(_) | RecipeBody::FurnaceMaterial(_) => {
                    view.shapeless.push(entry);
                    next += 1;
                }
                RecipeBody::SmithingTransform(_) => {
                    view.smithing_transform.push(entry);
                    next += 1;
                }
                RecipeBody::SmithingTrim(_) => {
                    // The vanilla trim recipe is hardcoded on the wire
                    // (id 1); custom trim bodies are not expressible in the
                    // trim segment, so they are disabled loudly.
                    if recipe.identifier == "minecraft:smithing_armor_trim" {
                        // Absorbed by the hardcoded entry; no extra id.
                        view.wire_disabled.push((
                            recipe.identifier.clone(),
                            "absorbed by hardcoded 2168 trim entry".to_string(),
                        ));
                    } else {
                        view.wire_disabled.push((
                            recipe.identifier.clone(),
                            "custom smithing trim has no 2168 wire encoding".to_string(),
                        ));
                    }
                }
                RecipeBody::BrewingMix(_) => {
                    view.brewing_mix.push(entry);
                    next += 1;
                }
                RecipeBody::BrewingContainer(_) => {
                    view.brewing_container.push(entry);
                    next += 1;
                }
                RecipeBody::MaterialReducer(_) => {
                    view.material_reducer.push(entry);
                    next += 1;
                }
            }
        }
        view
    }

    pub fn counts(&self) -> WireCounts {
        WireCounts {
            shaped: self.shaped.len(),
            shapeless: self.shapeless.len(),
            smithing_transform: self.smithing_transform.len(),
            brewing_mix: self.brewing_mix.len(),
            brewing_container: self.brewing_container.len(),
            material_reducer: self.material_reducer.len(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireCounts {
    pub shaped: usize,
    pub shapeless: usize,
    pub smithing_transform: usize,
    pub brewing_mix: usize,
    pub brewing_container: usize,
    pub material_reducer: usize,
}

/// Classify a recipe kind into its 2168 segment name (diagnostics).
pub fn segment_for(kind: RecipeKind) -> &'static str {
    match kind {
        RecipeKind::Shaped => "shaped",
        RecipeKind::Shapeless | RecipeKind::Furnace | RecipeKind::FurnaceMaterial => "shapeless",
        RecipeKind::BrewingMix => "brewing",
        RecipeKind::BrewingContainer => "container",
        RecipeKind::SmithingTransform => "smithing_transform",
        RecipeKind::SmithingTrim => "smithing_trim",
        RecipeKind::MaterialReducer => "material_reducer",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CompileBudgets, SourceRecipe};
    use crate::registry::RecipeRegistrySnapshot;

    fn snap(raws: Vec<serde_json::Value>) -> RecipeRegistrySnapshot {
        let sources: Vec<SourceRecipe> = raws
            .into_iter()
            .enumerate()
            .map(|(i, raw)| {
                let bytes = serde_json::to_vec(&raw).unwrap();
                SourceRecipe::new("p", 0, &format!("{i}.json"), "1.12", raw, &bytes)
            })
            .collect();
        let (snap, _) =
            RecipeRegistrySnapshot::compile(&sources, &CompileBudgets::default(), false).unwrap();
        snap
    }

    #[test]
    fn segments_split_by_kind() {
        let snapshot = snap(vec![
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shaped": {
                    "description": {"identifier": "minecraft:a"},
                    "tags": ["crafting_table"],
                    "pattern": ["X"], "key": {"X": {"item": "minecraft:stone"}},
                    "result": {"item": "minecraft:out"}
                }
            }),
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_furnace": {
                    "description": {"identifier": "minecraft:b"},
                    "tags": ["furnace"],
                    "input": "minecraft:stone",
                    "output": "minecraft:out"
                }
            }),
        ]);
        let view = CraftingWireView::build(snapshot.in_network_order(), 2);
        assert_eq!(view.shaped.len(), 1);
        assert_eq!(view.shapeless.len(), 1);
    }
}
