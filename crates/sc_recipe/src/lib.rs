//! Bedrock 配方系统：与协议无关的编译、匹配与合成。
//!
//! 数据流：`behavior_packs/<pack>/recipes/**/*.json`（sc_packloader::recipe_source）
//! -> SourceRecipe -> compile_ordered（按类型 schema、预算、隔离）->
//! RecipeRegistrySnapshot（不可变、确定序 + 指纹）-> matching（shaped / shapeless /
//! process）-> transaction（CraftIntent -> CraftReceipt，幂等、原子）->
//! wire（CraftingData 分段，与协议无关）-> unlock（recipesUnlock 规则 + 配方书）。
//!
//! Stage-1 的物品模型只有标识符 + `data`/损伤值 + 数量 + 标签。需要完整物品
//! 组件或 NBT 的配方会被带上原因禁用，不会忽略条件参与匹配。
//!
//! Bedrock recipe system: protocol-independent compile, match and craft.
//!
//! Data flow: `behavior_packs/<pack>/recipes/**/*.json` (sc_packloader::recipe_source)
//! -> SourceRecipe -> compile_ordered (per-kind schemas, budgets, quarantine) ->
//! RecipeRegistrySnapshot (immutable, deterministic order + fingerprint) ->
//! matching (shaped / shapeless / process) ->
//! transaction (CraftIntent -> CraftReceipt, idempotent, atomic) ->
//! wire (CraftingData segments, protocol-agnostic) ->
//! unlock (recipesUnlock rule + recipe book).
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
    compile_ordered, compile_source, fits_inventory_grid, registry_fingerprint, BrewingBody,
    CompileBudgets, CompileError, CompileOutput, CompiledRecipe, DisabledRecipe, FurnaceBody,
    MaterialReducerBody, RecipeBody, ShapedBody, ShapelessBody, SmithingTransformBody,
    SmithingTrimBody, SourceRecipe,
};
pub use kind::{RecipeKind, StationKind};
pub use matching::{
    find_best_instant, item_family_matches, match_process_input, match_shaped, match_shapeless,
    plan_consumption, rename_alias, MapTagResolver, MatchInput, TagResolver,
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
