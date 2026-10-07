# SculkBedrock — Developer Guide (DOCS)

> Language: English | [简体中文](DOCS_zh_CN.md)
>
> Public developer companion to [README.md](README.md).
## 0. Contents

1. [Prerequisites & repo map](#1-prerequisites--repo-map)
2. [Build, test, run](#2-build-test-run)
3. [Startup pipeline](#3-startup-pipeline)
4. [Version-pack pipeline](#4-version-pack-pipeline)
5. [Block / item / recipe data pipeline](#5-block--item--recipe-data-pipeline)
6. [Chunk pipeline](#6-chunk-pipeline)
7. [Network & protocol pipeline](#7-network--protocol-pipeline)
8. [Worldgen plugin pipeline](#8-worldgen-plugin-pipeline)
9. [Plugin development](#9-plugin-development)
10. [Persistence pipeline](#10-persistence-pipeline)
11. [Verification & diagnostics](#11-verification--diagnostics)
12. [Current capabilities vs. roadmap](#12-current-capabilities-vs-roadmap)
13. [Contribution rules](#13-contribution-rules)

## 1. Prerequisites & repo map

- Stable Rust toolchain with Cargo.
- A version pack (`.scver`) from <https://github.com/SculkBedrock/SculkBedrock-VersionPacks>, placed under `version_packs/`. Plugin sources for the pack live under `crates/ur_vanilla` in that repository.
- Default UDP port `19132`; config in `server_properties.toml`; saves under `worlds/`; logs under `logs/`.

```text
crates/
  sc_bootstrap/   entry point (sc_bootstrap::run) + startup systems
  sc_ecs/         custom ECS (World, resources, schedules, regions)
  sc_game/        gameplay (regions, interaction, intents, recipes view)
  sc_world/       chunks, executors, LevelDB, writeback, dictionaries
  sc_network/     protocol codec, connections, chunk pipeline, hooks
  sc_raknet/      RakNet transport, recovery queues
  sc_block/       block registry + compiled block-JSON snapshots
  sc_item/        item registry + inventories
  sc_recipe/      recipe snapshot compiler
  sc_packloader/  version-pack I/O, schemas, budgets, diagnostics
  sc_plugin/      plugin manager + loaders (rust / C-ABI)
  sc_plugin_api/  stable C-ABI host table (HostApiV1)
  sc_vanilla/     vanilla generator / command plugin sources
  sc_*/           binary, NBT, utils, logging, console, molang, entity…
tools/            pack & codegen helpers (see §4)
server_properties.toml / worlds/ / version_packs/ / logs/
```

## 2. Build, test, run

The workspace is large. Use limited parallelism everywhere:

```shell
cargo check -p sc_bootstrap -j 2
cargo run -p sc_bootstrap -j 2
cargo test -p sc_world -j 1 --lib
cargo test -p sc_network -j 1 --lib
cargo test -p sc_block -j 1 --lib
cargo test -p sc_packloader -j 1 --lib
```

Conventions:

- Prefer a targeted `cargo check` / `cargo test` for the crate you touched.
- Full-workspace builds and the live server are heavy; don't run them casually.
- `rustfmt --check` and `git diff --check` should pass for touched files.

## 3. Startup pipeline

Entry: `crates/sc_bootstrap/src/main.rs` → `sc_bootstrap::run()`. Systems run in schedule order:

```text
PreStartup:  init_log, init_properties
               (log sink + server_properties.toml → ServerProperties resource,
                console language, SCULK_LOCALE for dlopened plugins)
SCStartup:   startup
               (signal handlers, file sink, logger, StartupTimestamp, Server::init)
SCPreLoad:   packloader load_version
               (parse .scver → SCVersionPack resource: manifest, runtime ids,
                block bundle/blocks, items, recipes, worldgen, plugins…)
SCLoad:      load_block_bundle      (compile block JSON → immutable snapshot,
                                      atomic publish; never mixes old/new sources)
             load_block_palette     (legacy palette path when no block_data declared)
             load_item_registry     (runtime entries → ItemRegistry; block items
                                      link to default-state hash; air/shield checks)
             load_recipe_registry   (behavior-pack recipes → immutable snapshot +
                                      ingredient-group table filtered by registry)
SCPostLoad:  load_version_pack_plugins (kind:"rust" via builtin factories,
                                        kind:"cabi" via dlopen + HostApiV1)
             release_version_pack_bulk_data (free worldgen/tags/palette bytes,
                                              keep manifest + taken-empty tables)
PostStartup: finish_startup (log boot ms)
PostUpdate:  check_shutdown_signal (SIGINT/SIGTERM/SIGHUP → SCExit → graceful exit)
```

Shutdown is event-driven (`SCExit`): region threads stop, workers drain bounded results, dirty data gets a budgeted flush with an explicit unconfirmed-writes report, then the DB owner closes. A bounded wait never kills a thread mid-DB-write; it reports instead.

## 4. Version-pack pipeline

A pack is a `.scver` zip. Conceptually:

```text
manifest.json            (pack identity, protocol/game version, block_data decl)
definitions/
  blocks/**/*.block.json (one file per block type, vanilla shape + sc: extensions)
  runtime.json           (item name → network id)
  block_palette.nbt      (legacy path only)
  block_tags.json        (pack-level tags)
  recipe_groups.json     (ingredient groups, filtered by registry at boot)
  biomes/, entity_identifiers.nbt, worldgen/…
behavior_packs/          (recipes, item components…)
plugins/                 (*.scplugin zips: manifest.json + plugin.so, or kind:"rust")
```

Key rules:

- Declaring `block_data` switches the server to the new block-JSON path; old and new block sources are never mixed. Any bundle failure is loud — no silent fallback to air/stone.
- `tools/inject_block_json_pack.py` injects generated `.block.json` files and writes the manifest declaration.
- `tools/rewrite_block_pack.py`, `tools/blockgen_*.py` extract/derive block data (defaults, hardness, drops tables) from reference sources into pack files.
- `tools/gen_recipe_groups.py` regenerates the vendored ingredient-group table.
- `tools/package_sc_vanilla_macos.py` (and the `.ps1` twin) rebuilds pack-side Rust DLLs.
- **Lockstep rebuild rule:** any change to a cross-DLL-visible resource layout (e.g. adding fields to `SCVersionPack`) requires rebuilding every Rust DLL plugin in the pack, otherwise the DLL side resolves `None` for the resource. C-ABI tables are unaffected unless the table itself changes (which bumps ABI version).

## 5. Block / item / recipe data pipeline

```text
.block.json files
 → packloader schema parse (paths, budgets, duplicate-key rejection,
                             canonical state enumeration, restricted
                             q.block_property(...) == ... conditions)
 → block compiler (private builder → dense per-state tables:
                   collision, mining seconds, unbreakable, light,
                   loot refs, replaceable/liquid/random-tick/…, mining/drop profiles)
 → atomic publish (BlockJsonRegistry Arc snapshot + BlockStateRegistry +
                    capability flags + legacy palette adapter bytes)
 → game consumption (mining ticks = ceil(seconds*20), break progress
                     increments, drop rolls after commit)
```

- States are the Cartesian product of declared property values in canonical order; `sc:protocol_runtime_ids` align 1:1 and always come from the extraction source, never recomputed.
- `permutations` replace whole component values per matched state; overlapping covers on the same state fail the bundle.
- Items: `runtime.json` entries become `ItemRegistry`; identifiers hitting the block dictionary become block items linked to the default-state hash. Missing air is a corrupt-pack error.
- Recipes: behavior-pack sources compile low→high pack order (same-layer duplicates are hard errors); unknown recipe types are rejected; the ingredient-group table is filtered against the live item registry.

## 6. Chunk pipeline

### 6.1 Stages

```text
View Planner (desired deltas, distance + age priority, view/context tickets)
  → Chunk Coordinator (per-world-instance per-key dedup, budgets, job state)
  → Chunk Executor (bounded I/O + generation workers → private results)
  → owner publish (identity check → authoritative column)
  → Encode Coordinator (shared encode by column identity + wire profile)
  → Delivery Pump / ConnectionGate (hooks, barriers, tick/byte admission)
  → player used/spawn progress (only on Queued receipts)
Writeback Coordinator runs independently (snapshots → DB owner → SaveAck).
```

Three windows stay separate: **desired** (target keys), **prepare** (bounded approved load/encode demand), **delivery** (small ready set awaiting permits). A radius-10 view must never submit hundreds of jobs at once.

### 6.2 Identities and states

Track at least: world instance, connection session, context epoch (teleport/dimension barrier), view revision (ordinary moves keep overlap), column incarnation + content generation, job attempt, wire profile. Payload-cache identity = world + dimension/coord + incarnation + generation + profile.

Column lifecycle: `Absent → Queued → Loading/Generating → Applying → Ready`, with `Failed/Backoff` and `Closing`. Per-player key progress: `Wanted → AwaitingColumn → AwaitingPayload → Preparing → ReadyToDeliver → Admitted → SentForContext`, plus released/cancelled/failed. Loading-ready, encoding-ready, network-admitted, and client-initialized are four different facts.

### 6.3 Rules for contributors

- Same-world same-key demand coalesces; cache hits bypass workers.
- Every accepted job ends in success/failure/cancelled/worker-fault/closed; panics are isolated.
- Spillover uses total-order arbitration by operation ID (never arrival order) with authoritative-instance checks.
- Baselines pass final content/context/budget checks before entering the ordered per-connection chain; gaps fall back to full re-encode, never unbounded deltas.
- Budgets to respect: per-tick per-player effective admissions (default 4), spawn threshold validated against the reachable set (default 56), prepare window (default 12), recovery-queue count + byte + age limits, dirty high-water marks that suspend new generation.

## 7. Network & protocol pipeline

```text
Bedrock packet → decoder (per-connection ProtocolProfile)
 → boundary request (protocol-independent)
 → owner routing → validation + plugin decisions → commit
 → authoritative fact + receipt → NetworkIntent
 → intent hooks → packet translation → packet hooks
 → ConnectionGate (ordered, budgeted) → RakNet (reliable)
```

- Two hook levels: **intent hooks** (game semantics, can withdraw) and **packet hooks** (protocol semantics: checks, anti-cheat, audit). Outcomes include `Admitted / Cancelled / SuppressedByDebug / Busy / StaleContext / Closed / Failed`. Never hold ECS/chunk/view locks across hook `await`s.
- `Admitted` means entry into the reliable ordered chain — not UDP sent, not client rendered.
- Player lifecycle: connect → login sequence → first-spawn flow (creative content, inventory, spawn threshold) → `InGame` → movement/interaction intents → teleport/reset barriers → disconnect (list/remove broadcasts, lease cleanup).
- Adding a packet: add codec + profile mapping + golden fixture; never change layout without a versioned profile and fixture update.

## 8. Worldgen plugin pipeline

```text
pack worldgen bundle + seed + generator descriptor
 → vanilla generator plugin validates/compiles immutable runtime
 → bounded generation job on worker
 → owner validates identity → publishes ChunkColumn
```

- C-ABI generators register via `HostApiV1::register_world_generator(world_kind, generate)`, receive `ChunkGenRequest { x, z, dimension, min_y, max_y }`, and write through the `set_block_cb` callback into the chunk under construction.
- Block hashes for generation use the canonical `name+states` FNV1a-32 contract.
- Saves record generator identity; opening an old save keeps its descriptor. Never silently change terrain for existing saves.

## 9. Plugin development

| Kind | Packaging | Execution |
|---|---|---|
| `rust` | manifest-only entry in pack `plugins/`; code compiled into the server; matched by name to builtin factories | in-process, full ECS access, must use owner-scoped commands |
| `cabi` (default) | ̀*.scplugin` zip with `manifest.json` + `plugin.so/.dll/.dylib` | dlopened, only `sc_plugin_api` (`HostApiV1`: generators, block hash, logging) |

Lifecycle: dependency check → enable → register systems/generators → on disable/unload: reject new callbacks, unregister, cancel/drain tasks, then destroy context/unload the library. Breaking host-table layout changes require a major/negotiated ABI bump — never silently extend V1.

## 10. Persistence pipeline

```text
owner snapshot(g) → release locks → bounded WritebackQueue
 → DB-owner thread persists → SaveAck(key, g, result)
 → clear only dirty ≤ g when nothing newer changed
```

- One owner thread per LevelDB instance; game threads never touch DB objects.
- Old snapshots never overwrite newer versions; failures keep data dirty with bounded backoff.
- Spillover is a gameplay fact with a bounded journal mindset: stable op IDs, dedup, replay, target-column application, cleanup points — never silently dropped.
- Unload/shutdown order: stop admission → cancel consumerless visual jobs → drain completions → finish transactions/journals → budgeted flush with per-key `SaveAck` checks → close DB owner → allow reopen (new instance ID).

## 11. Verification & diagnostics

```shell
cargo test -p sc_world -j 1 --lib        # storage, executor, spillover, writeback
cargo test -p sc_network -j 1 --lib      # encoder, delivery barrier, budgets
cargo test -p sc_block -j 1 --lib       # snapshots, real-pack equivalence
cargo test -p sc_packloader -j 1 --lib  # schemas, conditions, budgets
cargo test -p sc_game -j 1 --lib        # interaction, outbox admission
```

- Real-pack tests compile the full block set and compare state-by-state against the legacy palette; mining coverage is logged at boot (declared / unbreakable / fallback counts).
- Scenario coverage spans spillover determinism, context/teleport barriers, overlap-preserving moves, shared demand dedup, world isolation, reopen fencing, bounded-queue terminal states, worker faults, hook cancellation semantics, encode/save races, spawn-threshold diagnostics, retransmit budgets, slow-connection handling, dirty/backpressure behavior, bounded shutdown, restart journals, login flow, save round-trips with unknown NBT, and seed/ordering determinism.
- What unit tests cannot replace (needs a real client, real disk, or power events): RSS/allocator/throughput baselines, full login→chunk→teleport flows, slow client/disk behavior, crash/power recovery.
- Observe via `logs/`, per-region tick stats, queue bytes/oldest-age, transaction accept/prepare/commit/abort counters, desired/prepare/ready/admitted distributions, per-connection admitted bytes, hook latency, dirty bytes/age, `SaveAck` versions, and allocator/RSS readings.

## 12. Status checklist

All `[x]` items are partial and not production acceptance.

### Completed

**Core framework**

- [x] Custom ECS with worlds, resources, and schedules
- [x] Startup pipeline (config → version pack → registries → plugins → listen)
- [x] Version-pack loading with strict schema validation and loud failures

**Blocks**

- [x] Per-block JSON definitions with canonical state enumeration and atomic publish
- [x] Dense capability tables (collision, light, replaceable, random-tick filter, …)
- [x] Server-authoritative break timing (`ceil(seconds × 20)`, unbreakable denied in survival)

**Chunks**

- [x] View tracking with overlap preservation on ordinary moves and hard-context resets
- [x] Shared demand coalescing (same world + key) with bounded executors
- [x] Shared encoding by content identity and wire profile
- [x] Deterministic spillover arbitration by operation ID

**Network**

- [x] Ordered per-connection delivery with explicit receipts (`Queued` advances progress)
- [x] Two-level outbound admission (overwritable state vs reliable facts)
- [x] Intent hooks + packet hooks with `StaleContext` barriers for teleport/dimension changes
- [x] RakNet retransmit budgets; recovery queue with count + byte + age limits

**Persistence**

- [x] Single-owner LevelDB thread with background bounded writeback
- [x] Versioned `SaveAck` confirmations; dirty kept on failure
- [x] Bounded shutdown with unconfirmed-writes report

**Safety rails**

- [x] Bounded log queue, decompression-output cap, oversized-NBT rejection

### Uncompleted

**Redstone**

- [ ] Redstone dust power propagation
- [ ] Repeaters, comparators, and timing behavior
- [ ] Pistons (including cross-chunk / cross-region movement)
- [ ] Redstone-driven multi-block coordination across Regions
- [ ] Random-tick scheduling for redstone components

**World generation**

- [ ] Density-function / noise-router data-driven pipeline from the version pack
- [ ] Biome selection and surface rules wired end-to-end
- [ ] Generator identity recorded into saves (fingerprint wiring)
- [ ] Same seed + same profile ⇒ identical terrain guarantee
- [ ] Structure / tree placement with restart-safe journals

**Blocks**

- [ ] BlockEntity landing (coordinate index, idempotent assembly, lifecycle-aware unload)
- [ ] `behaviors` bindings for special blocks (doors, beds, containers, …)
- [ ] External loot-table resolution
- [ ] Tool-specific mining rules, harvest gates, and fortune tables
- [ ] Silk-touch replacement, item tags, durability, XP drops

**Items & inventory**

- [x] Item registry with block-item mapping and creative inventory fill
- [ ] Inventory reservations and cross-owner transactions
- [ ] Full container transactions with rollback and resync
- [ ] Durability / unbreaking consumption
- [ ] Adventure-mode `CanDestroy` lists

**Entities**

- [x] Basic movement, physics, and broadcast
- [ ] Authoritative resync for entity lifecycle facts (spawn / despawn / pickup)
- [ ] Cross-region migration with fence / ack / epoch (production-hardened)
- [ ] Drops and pickup with budgets, cooldowns, and full-inventory handling

**Chunk pipeline**

- [ ] Full prepare / ready / delivery window separation in the live send path
- [ ] Per-connection / world / server wire-byte budgets and fairness
- [ ] Complete ordered `ConnectionGate` with spawn / teleport barriers
- [ ] Bounded delta journals for hot columns (only gap counters exist today)

**Network & protocol**

- [ ] Multi-protocol-profile isolation (one pack supports one profile today)
- [ ] Full login → chunk → teleport flows verified on a real client
- [ ] Real slow-client behavior and hook-timeout fault policy

**Persistence & recovery**

- [ ] Crash / power-loss recovery (WAL durability exists; recovery unverified)
- [ ] Restart-safe spillover journals with replay and cleanup points
- [ ] Dirty pin byte / age budgets and storage-I/O timeouts

**Plugins**

- [x] Dual channels: in-process Rust plugins + C-ABI dynamic plugins
- [ ] Explicit versioned bundle API for data-driven definitions
- [ ] ABI capability negotiation and safe unload draining (hardened paths)

**Performance baselines**

- [ ] RSS / allocator / throughput / tick-latency baselines with real clients
- [ ] Profile-driven memory and CPU optimization (NBT sharing, payload accounting, indexing)

## 13. Contribution rules

1. Regions own mutation; locks are memory protection, not write permission.
2. No global single-thread submitter for world mutations; order within owners, parallel across them.
3. Snapshot data before `await`; release all guards first; never hold locks across Regions/`await`.
4. Make ordering explicit; never rely on registration order, hash-map order, or worker finish order.
5. Keep game/network, data/behavior, and format/contract boundaries; version packs are data, not code.
6. Bound everything: count, bytes, in-flight, age, backpressure, cancel, drain.
7. Smallest change consistent with the target design; add focused tests for ownership, stale-result rejection, ordering, retries, saturation, cancellation, and save acknowledgements.
