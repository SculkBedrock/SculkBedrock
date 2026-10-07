# Agent Instructions for SculkBedrock

## Start here

- Read [`docs/unified_architecture_design_2026-09-30.md`](docs/unified_architecture_design_2026-09-30.md) before changing architecture, concurrency, world, chunk, block/item, protocol, persistence, or plugin code.
- For chunk-pipeline details and its dated implementation snapshot, also read [`docs/performance_memory_and_chunk_pipeline_refactor_2026-09-30.md`](docs/performance_memory_and_chunk_pipeline_refactor_2026-09-30.md).
- The unified design is a **target architecture**, not proof that every feature is implemented. Verify current code, tests, and status before describing or changing implementation behavior.
- The old standalone design files were deliberately consolidated. Do not restore or recreate them unless the user asks.

## Architecture rules

1. **Region ownership is the concurrency boundary.** Each mutable entity/chunk has one authoritative owner at a time. Locks protect memory access; they do not grant permission to mutate another Region's state.
2. **Do not funnel all world mutations through a global single-thread submitter.** Keep commands ordered within their owner/Region and allow independent Regions to apply concurrently.
3. **A Region is not an OS thread or necessarily an ECS `World`.** Use a bounded scheduler; do not assume one permanent thread or one full ECS app per Region. Keep storage sharding an implementation decision guided by measurements.
4. **Workers prepare; owners publish.** IO, generation, encoding, and other background work must return owned results carrying the relevant world/owner/job/content identity. The owning Region validates those identities before publishing world state.
5. **Cross-Region communication uses bounded value messages.** Never hold locks across Regions or across `await`. Multi-owner changes need an operation ID, explicit prepare/commit/abort and receipts; do not rely on thread completion order.
6. **Keep the game/network boundary.** Game logic uses protocol-independent requests and intents; `sc_network` owns wire encoding, hooks, connection state, and per-connection ordering. Do not make gameplay state depend on packet-send success without an explicit receipt contract.
7. **Preserve data-driven block/item design.** Blocks remain paletted chunk data, not one ECS entity per block. Use dense state/property tables for static capabilities and ECS entities for stateful BlockEntities. Keep item IDs, block runtime IDs, and internal dense IDs distinct.
8. **Treat protocol and persistence formats as compatibility contracts.** Use the connection's protocol profile and existing Bedrock storage semantics. Do not change packet layouts, palette/hash rules, LevelDB keys, or unknown-NBT round-tripping without explicit tests and migration analysis.
9. **Version world generation.** Do not silently change terrain for an existing save when a generator, seed stream, pack, or settings change. Keep generator identity/configuration explicit and deterministic.
10. **Critical facts need reliable bounded paths.** Ordinary ECS Events and telemetry are not durable queues. Accepted gameplay changes, migrations, write acknowledgements, and transaction completions require bounded admission, explicit terminal receipts, and a failure/recovery policy.
11. **Every queue/cache/worker needs limits.** Consider count, bytes, in-flight work, age, backpressure, cancellation, and shutdown draining. Never solve overload by unbounded spawning or silently dropping dirty/gameplay data.
12. **Plugins must respect ownership and lifecycle.** Region-facing APIs should be owner-scoped. Do not silently change a C-ABI table while keeping the same ABI version; drain callbacks/tasks before destroying plugin state or unloading its library.

## Concurrency and implementation practice

- Before editing, inspect the owning crate's current API and callers; make the smallest change consistent with the target design.
- Snapshot the data needed by async work, then release ECS/resource/component/chunk/view/cache guards before awaiting.
- Make ordering dependencies explicit. Do not rely on incidental plugin registration order, hash-map iteration order, or worker completion order for correctness.
- Distinguish a target design, a proposed change, an implemented change, and a verified behavior in code and documentation.
- Preserve existing user changes. Do not reset, revert, overwrite, or clean unrelated work in the working tree.
- Avoid destructive repository operations unless the user specifically requests them.

## Validation

- Add or update focused tests for ownership, stale-result rejection, ordering, retries, queue saturation, cancellation, and persistence acknowledgements when changing those paths.
- Prefer targeted `cargo check`/`cargo test` for the affected crate. Use limited parallelism (`-j 1` or `-j 2`) unless the user requests otherwise; consult `memory.md` before broad builds or running the server.
- For protocol or storage changes, test the exact protocol/save profile and preserve golden fixtures or round-trip coverage. Do not claim real-client, crash-recovery, memory, or throughput validation unless it was actually performed.
- If validation cannot run, state exactly what was and was not checked.
