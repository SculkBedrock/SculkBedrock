//! Bedrock recipe system: protocol-independent compile, match and craft.
//!
//! Data flow:
//! ```text
//! behavior_packs/<pack>/recipes/**/*.json (sc_packloader::recipe_source)
//!   -> SourceRecipe (this crate)
//!   -> compile_ordered (per-kind schemas, budgets, quarantine)
//!   -> RecipeRegistrySnapshot (immutable, deterministic order + fingerprint)
//!   -> matching (shaped / shapeless / process)
//!   -> transaction (CraftIntent -> CraftReceipt, idempotent, atomic)
//!   -> wire (CraftingData segments, protocol-agnostic)
//!   -> unlock (recipesUnlock rule + recipe book)
//! ```
//!
//! Stage-1 item model: identifier + `data`/damage + count + tag. Anything
//! needing full item components / NBT is disabled with a reason, never
//! matched by ignoring the condition.

pub mod compile;
pub mod kind;
pub mod matching;
pub mod registry;
pub mod spec;
pub mod transaction;
pub mod unlock;
pub mod wire;

pub use compile::{
    compile_ordered, compile_source, registry_fingerprint, BrewingBody, CompileBudgets,
    CompileError, CompileOutput, CompiledRecipe, DisabledRecipe, FurnaceBody, MaterialReducerBody,
    RecipeBody, ShapedBody, ShapelessBody, SmithingTransformBody, SmithingTrimBody, SourceRecipe,
};
pub use kind::{RecipeKind, StationKind};
pub use matching::{
    find_best_instant, item_family_matches, match_process_input, match_shaped, match_shapeless,
    plan_consumption, MapTagResolver, MatchInput, TagResolver,
};
pub use registry::{
    CompileOutputDiagnostics, RecipeId, RecipeIndex, RecipeRegistryFingerprint,
    RecipeRegistrySnapshot,
};
pub use spec::{IngredientChoice, IngredientSpec, OutputSpec, DATA_WILDCARD};
pub use transaction::{
    decide_commit, execute_craft, station_allows, CommitChecks, CraftActor, CraftContext,
    CraftIntent, CraftQueue, CraftReceipt, CraftReject, ExpectedSlot, RegionReservation,
    SeenOperations, TxInventory, TxSlot, TxnDecision,
};
pub use unlock::{PlayerRecipeBook, UnlockPolicy};
pub use wire::{segment_for, CraftingWireView, WireCounts, WireRecipe};
