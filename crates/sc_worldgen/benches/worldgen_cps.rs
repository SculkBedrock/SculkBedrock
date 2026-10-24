//! sc_worldgen 吞吐基准：分别测量单线程 CPS 与多线程 CPS。
//!
//! Throughput benchmark for sc_worldgen: measures single-threaded CPS and
//! multi-threaded CPS separately.
//!
//! 口径（与 `docs/worldgen_single_thread_optimization_audit_2026-10-20.md`
//! 的直接生成口径一致）：每轮在计时窗口内对一个区块位置调用生成入口，
//! 结果在循环内析构；不含存储写盘、网络编码、区块 provider 排队。
//!
//! Measurement boundary (matching the direct-generation boundary used by
//! `docs/worldgen_single_thread_optimization_audit_2026-10-20.md`): each round
//! calls the generation entry once per chunk position inside the timed window
//! and drops the result inside the loop. Storage commits, network encoding and
//! provider queueing are outside the boundary.
//!
//! 单线程与多线程使用**同一组**区块坐标（从 `--origin` 起的方形螺旋），
//! 因此两条腿的算术量相同，CPS 可直接比较。
//!
//! Single- and multi-threaded legs use the **same** chunk coordinate list (a
//! square spiral from `--origin`), so both legs do identical arithmetic and the
//! CPS numbers are directly comparable.
//!
//! 真实数据来自版本包 `definitions/worldgen/`（density_function / noise /
//! noise_settings）；不提供合成后备——找不到数据就响亮失败。
//!
//! Real data comes from the version pack's `definitions/worldgen/`
//! (density_function / noise / noise_settings). There is no synthetic fallback:
//! missing data fails loudly.
//!
//! 用法 / Usage:
//!
//! ```text
//! cargo bench -p sc_worldgen --bench worldgen_cps
//! cargo bench -p sc_worldgen --bench worldgen_cps -- --threads 1,2,4,8 --chunks 128
//! cargo bench -p sc_worldgen --bench worldgen_cps -- --stage base --verify
//! ```
//!
//! 多线程腿的每轮区块数不要太少：每轮有屏障同步与线程起步成本，`--chunks 64`
//! 在 11 线程下会低估约 15%（1285 CPS vs 256 区块/轮的 1518 CPS）。
//!
//! Keep per-round chunk counts reasonably large for multi-threaded legs: each
//! round pays barrier/ramp cost, and `--chunks 64` under-reports 11-thread CPS
//! by ~15% versus 256 chunks/round.
//!
//! 必须用 `--release`（`cargo bench` 默认 bench profile）运行：debug 构建的
//! 噪声求值比 release 慢一个数量级以上，CPS 没有参考价值。
//!
//! Always run in the bench/release profile: debug-build noise evaluation is
//! more than an order of magnitude slower and its CPS is meaningless.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_world::chunk::{Chunk, ChunkPosition, PalettedBlockStorage};
use sc_world::storage::{
    ChunkGenerationRequest, ChunkKey, WorldGenerator, MAX_GENERATED_SPILLOVER_WRITES,
};

use sc_worldgen::{
    BaseBlockPalette, BaseTerrainGenerator, BuiltinWorldGenerator, CompiledWorldgen, Node,
    SurfaceBlockPalette, WorldgenSources,
};

/// 受测的生成入口。
///
/// Generation entry under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// 完整内置生成：基地形 + 生物群系 + 地表修饰 + 特征（生产入口）。
    ///
    /// Full built-in generation: base terrain + biomes + surface dressing +
    /// features (the production entry point).
    Full,
    /// 仅基地形：密度求值 + 三线性插值 + 方块填充。
    ///
    /// Base terrain only: density evaluation + trilinear interpolation + fill.
    Base,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Base => "base",
        }
    }

    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "full" => Ok(Self::Full),
            "base" => Ok(Self::Base),
            other => Err(format!("unknown --stage `{other}` (expected `full` or `base`)")),
        }
    }
}

/// 命令行配置。
///
/// Command-line configuration.
struct Config {
    pack: Option<PathBuf>,
    seed: i64,
    chunks: usize,
    rounds: usize,
    warmup_rounds: usize,
    threads: Vec<usize>,
    stage: Stage,
    origin: (i32, i32),
    verify: bool,
    /// 打印地形「非 flat」证据（计划节点普查 + 表面高度/方块直方图 + 单列剖面）。
    ///
    /// Print evidence that the compiled assets are real terrain (plan census +
    /// surface-height/block histograms + one column profile).
    probe: bool,
    /// 只跑探针、不跑计时腿（`--probe-only`）。
    ///
    /// Run only the probe, skipping the timed legs (`--probe-only`).
    probe_only: bool,
    json: bool,
    /// IDE（RustRover/CLion）以 `--format=json` 请求 libtest 风格事件流。
    ///
    /// Set when an IDE passes `--format=json`, requesting libtest-style events.
    ide_json: bool,
    /// 位置参数名字过滤（子串匹配 leg 名，供 IDE「重跑单条腿」使用）。
    ///
    /// Positional name filters: substring matches against the leg name, so an
    /// IDE can re-run a single leg.
    filters: Vec<String>,
    skips: Vec<String>,
    exact: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pack: None,
            seed: 0,
            chunks: 256,
            rounds: 3,
            warmup_rounds: 1,
            threads: vec![1, default_worker_threads()],
            stage: Stage::Full,
            origin: (0, 0),
            verify: false,
            probe: false,
            probe_only: false,
            json: false,
            ide_json: false,
            filters: Vec::new(),
            skips: Vec::new(),
            exact: false,
        }
    }
}

impl Config {
    fn parse<I: Iterator<Item = String>>(mut args: I) -> Result<Self, String> {
        let mut config = Self::default();
        let mut custom_threads = false;
        while let Some(arg) = args.next() {
            let mut value = |name: &str| -> Result<String, String> {
                args.next()
                    .ok_or_else(|| format!("`{name}` requires a value"))
            };
            match arg.as_str() {
                "--pack" => config.pack = Some(PathBuf::from(value("--pack")?)),
                "--seed" => {
                    config.seed = value("--seed")?
                        .parse()
                        .map_err(|e| format!("invalid --seed: {e}"))?
                }
                "--chunks" => {
                    config.chunks = value("--chunks")?
                        .parse::<usize>()
                        .map_err(|e| format!("invalid --chunks: {e}"))?
                        .max(1)
                }
                "--rounds" => {
                    config.rounds = value("--rounds")?
                        .parse::<usize>()
                        .map_err(|e| format!("invalid --rounds: {e}"))?
                        .max(1)
                }
                "--warmup" => {
                    config.warmup_rounds = value("--warmup")?
                        .parse()
                        .map_err(|e| format!("invalid --warmup: {e}"))?
                }
                "--threads" => {
                    config.threads = parse_threads(&value("--threads")?)?;
                    custom_threads = true;
                }
                "--stage" => config.stage = Stage::parse(&value("--stage")?)?,
                "--origin" => {
                    let raw = value("--origin")?;
                    let (x, z) = raw
                        .split_once(',')
                        .ok_or_else(|| format!("invalid --origin `{raw}` (expected `x,z`)"))?;
                    config.origin = (
                        x.trim()
                            .parse()
                            .map_err(|e| format!("invalid --origin x: {e}"))?,
                        z.trim()
                            .parse()
                            .map_err(|e| format!("invalid --origin z: {e}"))?,
                    );
                }
                "--verify" => config.verify = true,
                "--probe" => config.probe = true,
                "--probe-only" => {
                    config.probe = true;
                    config.probe_only = true;
                }
                "--json" => config.json = true,
                // `cargo bench` 会把 libtest 风格的标志透传给 bench 目标；
                // 本基准自建 harness，这些标志一律无副作用地忽略。
                //
                // `cargo bench` forwards libtest-style flags to the bench
                // target; this benchmark has its own harness, so they are
                // accepted and ignored.
                // 以下是 cargo / RustRover / CLion 透传的 libtest 风格标志：本基准
                // 自建 harness，接受并忽略它们。带值的成对标志必须把值一起吃掉，
                // 否则值会被当成本基准的名字过滤参数。
                //
                // libtest-style flags forwarded by cargo / RustRover / CLion: this
                // benchmark has its own harness, so they are accepted and ignored.
                // Value-taking flags must consume their value too, otherwise the
                // value would be read as a name filter.
                "--bench" | "--test" | "--nocapture" | "--ignored" | "--include-ignored"
                | "--quiet" | "--list" | "--show-output" | "--capture" => {}
                "--exact" => config.exact = true,
                "--format" => {
                    if let Some(value) = args.next() {
                        config.ide_json |= value == "json";
                    }
                }
                other if other.starts_with("--format=") => {
                    config.ide_json |= other.strip_prefix("--format=") == Some("json");
                }
                "-Z" => {
                    let _ = args.next();
                }
                other if other.starts_with("-Z") && other.len() > 2 => {}
                "--test-threads" | "--color" | "--logfile" | "--spawn" | "--module" => {
                    let _ = args.next();
                }
                "--skip" => {
                    if let Some(value) = args.next() {
                        config.skips.push(value);
                    }
                }
                "--run" | "--exclude" => {
                    let _ = args.next();
                }
                other
                    if other.starts_with("--test-threads=")
                        || other.starts_with("--color=")
                        || other.starts_with("--logfile=")
                        || other.starts_with("--spawn=")
                        || other.starts_with("--module=")
                        || other.starts_with("--run=")
                        || other.starts_with("--exclude=") => {}
                other if other.starts_with("--skip=") => {
                    config.skips.push(other["--skip=".len()..].to_string());
                }
                other if other.starts_with('-') => {
                    return Err(format!("unknown flag `{other}` (see --help)"));
                }
                other => config.filters.push(other.to_string()),
            }
        }
        if !custom_threads {
            config.threads = vec![1, default_worker_threads()];
        }
        // 去重但**保留用户给的顺序**：`--threads 11,1` 必须真的先跑 11 线程，
        // 否则「换序检查热漂移」的说法不成立。
        //
        // Deduplicate while preserving the user's order: `--threads 11,1` must
        // really run 11 threads first, otherwise the ordering-drift check would
        // be meaningless.
        let mut deduped: Vec<usize> = Vec::with_capacity(config.threads.len());
        for threads in std::mem::take(&mut config.threads) {
            if !deduped.contains(&threads) {
                deduped.push(threads);
            }
        }
        config.threads = deduped;
        if config.threads.is_empty() {
            return Err("--threads must list at least one thread count".to_string());
        }
        Ok(config)
    }
}

/// 生产默认工作任务数（与 [`sc_worldgen::WorldgenRuntime`] 的暂存池策略一致：
/// `available_parallelism - 1`，下限 2）。
///
/// Production default worker count (mirrors the scratch-pool policy of
/// [`sc_worldgen::WorldgenRuntime`]: `available_parallelism - 1`, floored at 2).
fn default_worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(2)
}

fn parse_threads(raw: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let count = match part {
            "max" => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
            other => other
                .parse::<usize>()
                .map_err(|e| format!("invalid --threads entry `{other}`: {e}"))?,
        };
        if count == 0 {
            return Err("--threads entries must be >= 1".to_string());
        }
        out.push(count);
    }
    Ok(out)
}

/// 帮助文本（逐行保留缩进；`\` 续行会吞掉行首空白，故不用续行）。
///
/// Help text, one line per entry so indentation survives (a `\` line
/// continuation would strip leading whitespace).
const USAGE_LINES: &[&str] = &[
    "sc_worldgen CPS bench — single-thread vs multi-thread chunk generation",
    "",
    "Usage: cargo bench -p sc_worldgen --bench worldgen_cps -- [options]",
    "",
    "Options:",
    "  --pack <dir>       version pack directory containing definitions/worldgen/",
    "                     (default: first pack found under <repo>/version_packs/)",
    "  --seed <i64>       world seed (default 0)",
    "  --chunks <n>       chunk positions generated per round (default 256;",
    "                     smaller values under-report multi-thread CPS because", 
    "                     per-round barrier/ramp cost is a larger fraction)",
    "  --rounds <n>       timed rounds per leg (default 3)",
    "  --warmup <n>       untimed warmup rounds per leg (default 1)",
    "  --threads <list>   thread counts to measure, e.g. `1,2,4,8` or `max`",
    "                     (default: `1,%DEFAULT_THREADS%` = 1 and available_parallelism - 1)",
    "  --stage <name>     `full` (production entry) or `base` (terrain only); default full",
    "  --origin <x,z>     spiral start chunk coordinate (default 0,0)",
    "  --probe-only       print non-flat evidence and exit (no timed legs)",
    "  --probe            print non-flat evidence: plan census, surface-height and",
    "                     block histograms, one column profile (uses min(chunks, 64))",
    "  --verify           hash every chunk and compare single vs multi-thread digests",
    "                     (adds hashing cost; throughput numbers then include it)",
    "  --json             emit this benchmark's own machine-readable JSON summary",
    "  --help             show this help",
    "",
    "libtest-style flags forwarded by cargo/IDEs (--bench, --format=json,",
    "-Z unstable-options, --show-output, --test-threads=N, ...) are accepted and",
    "ignored. With --format=json the run additionally emits one libtest-shaped JSON",
    "event per leg so the IDE test tree lists each leg. A bare argument filters leg",
    "names by substring (use --exact for equality).",
    "",
    "Both legs generate the same chunk coordinate list, so CPS is directly comparable.",
    "CPS = chunks / wall-clock of the generation loop (no storage, network or lighting).",
];

fn print_usage() {
    let default_threads = default_worker_threads().to_string();
    for line in USAGE_LINES {
        println!("{}", line.replace("%DEFAULT_THREADS%", &default_threads));
    }
}

// ---------------------------------------------------------------------------
// Version pack data loading
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    // <repo>/crates/sc_worldgen -> (0) sc_worldgen, (1) crates, (2) <repo>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("bench file must live under <repo>/crates/sc_worldgen/benches")
        .to_path_buf()
}

/// 定位版本包目录（含 `definitions/worldgen/noise_settings`）。
///
/// Locates a version pack directory (one containing
/// `definitions/worldgen/noise_settings`).
fn find_version_pack(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        if dir.join("definitions/worldgen/noise_settings").is_dir() {
            return Ok(dir.to_path_buf());
        }
        return Err(format!(
            "{} is not a version pack: missing definitions/worldgen/noise_settings",
            dir.display()
        ));
    }

    if let Some(dir) = std::env::var_os("SC_WORLDGEN_BENCH_PACK") {
        let dir = PathBuf::from(dir);
        if dir.join("definitions/worldgen/noise_settings").is_dir() {
            return Ok(dir);
        }
        return Err(format!(
            "SC_WORLDGEN_BENCH_PACK={} is not a version pack",
            dir.display()
        ));
    }

    let root = repo_root().join("version_packs");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|e| format!("cannot read {}: {e}", root.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.join("definitions/worldgen/noise_settings").is_dir())
        .collect();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        format!(
            "no version pack with definitions/worldgen/ found under {}; pass --pack <dir>",
            root.display()
        )
    })
}

/// 递归读取一个 worldgen 类别目录：键为 `<namespace>/<path>`（去掉 `.json`）。
///
/// Recursively reads one worldgen category directory, keyed by
/// `<namespace>/<path>` with `.json` stripped.
fn load_category(dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&current)
            .map_err(|e| format!("cannot read {}: {e}", current.display()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let relative = path
                .strip_prefix(dir)
                .map_err(|e| format!("{} is not under {}: {e}", path.display(), dir.display()))?;
            let key = relative
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            out.insert(key, bytes);
        }
    }
    Ok(())
}

fn load_sources(pack: &Path) -> Result<WorldgenSources, String> {
    let base = pack.join("definitions/worldgen");
    let mut sources = WorldgenSources::default();
    load_category(&base.join("density_function"), &mut sources.density_functions)?;
    load_category(&base.join("noise"), &mut sources.noises)?;
    load_category(&base.join("noise_settings"), &mut sources.noise_settings)?;
    if sources.is_empty() {
        return Err(format!(
            "{} contains no worldgen JSON (density_function/noise/noise_settings)",
            base.display()
        ));
    }
    Ok(sources)
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// 受测生成器：把两种入口统一成一个可跨线程共享的对象。
///
/// Generator under test: unifies both entries into one shareable object.
enum BenchGenerator {
    Full {
        generator: BuiltinWorldGenerator,
        min_y: i32,
        max_y: i32,
        /// `[oak_log, oak_leaves]` 运行时 id：用于廉价探测特性阶段是否
        /// 真的落下方块（只扫描调色板，不逐方块扫描）。
        ///
        /// `[oak_log, oak_leaves]` runtime ids: a cheap probe for whether the
        /// feature stage actually places blocks (palette scan only, no
        /// per-block scan).
        wood: [u32; 2],
    },
    Base {
        generator: BaseTerrainGenerator,
    },
}

impl BenchGenerator {
    fn new_full(compiled: &CompiledWorldgen, seed: i64) -> Self {
        let generator = BuiltinWorldGenerator::new(compiled, seed);
        let runtime = generator.runtime();
        let min_y = runtime.min_y() as i32;
        let max_y = min_y + runtime.height() as i32 - 1;
        let wood = [
            sc_world::block_dictionary::resolve_or_register_default("minecraft:oak_log"),
            sc_world::block_dictionary::resolve_or_register_default("minecraft:oak_leaves"),
        ];
        Self::Full {
            generator,
            min_y,
            max_y,
            wood,
        }
    }

    /// 仅基地形模式的生成器：复用完整生成器的运行时与描述符，只走
    /// `generate_base_chunk`。
    ///
    /// Base-only generator: reuses the full generator's runtime and descriptor
    /// and calls only `generate_base_chunk`.
    fn new_base(compiled: &CompiledWorldgen, seed: i64) -> Self {
        let full = BuiltinWorldGenerator::new(compiled, seed);
        let palette = SurfaceBlockPalette::default();
        let base_palette = BaseBlockPalette {
            stone: palette.stone,
            deepslate: palette.deepslate,
            bedrock: palette.bedrock,
            water: palette.water,
            air: palette.air,
        };
        let generator = BaseTerrainGenerator::new(
            Arc::clone(full.runtime()),
            0,
            1,
            full.descriptor().fingerprint(),
            base_palette,
        );
        Self::Base { generator }
    }

    /// 生成一个区块位置，返回观测值（不含计时）。
    ///
    /// Generates one chunk position and returns observations (no timing).
    ///
    /// - `keep_chunk`: 是否保留区块本体供 `--verify` 哈希。为 `false` 时结果
    ///   在本调用返回前析构，计时口径与普通生成一致。
    ///
    /// - `keep_chunk`: whether to keep the chunk for `--verify` hashing. When
    ///   `false` the result is dropped before this call returns, keeping the
    ///   timing boundary identical to plain generation.
    fn generate(&self, cx: i32, cz: i32, keep_chunk: bool) -> Result<Observed, String> {
        match self {
            Self::Full {
                generator,
                min_y,
                max_y,
                wood,
            } => {
                let request = ChunkGenerationRequest {
                    key: ChunkKey::new(0, ChunkPosition::new(cx, cz)),
                    min_y: *min_y,
                    max_y: *max_y,
                };
                let generated = generator
                    .generate_chunk_with_spillover_bounded(request, MAX_GENERATED_SPILLOVER_WRITES)
                    .map_err(|err| format!("full generation failed at ({cx},{cz}): {err}"))?;
                let Some(generated) = generated else {
                    return Err(format!("full generation returned None at ({cx},{cz})"));
                };
                let sections = generated.chunk.subchunks.len();
                let spillover = generated.spillover.len();
                let wood = has_wood(&generated.chunk, wood);
                Ok(Observed {
                    sections,
                    spillover,
                    wood,
                    chunk: keep_chunk.then_some(generated.chunk),
                })
            }
            Self::Base { generator } => {
                let snapshot = generator
                    .generate_base_chunk(ChunkPosition::new(cx, cz))
                    .map_err(|err| format!("base generation failed at ({cx},{cz}): {err}"))?;
                let sections = snapshot.chunk.subchunks.len();
                Ok(Observed {
                    sections,
                    spillover: 0,
                    wood: false,
                    chunk: keep_chunk.then(|| (*snapshot.chunk).clone()),
                })
            }
        }
    }
}

/// 单次生成的观测值。
///
/// Observations from one generation call.
struct Observed {
    sections: usize,
    spillover: usize,
    /// 该区块调色板中是否出现过原木/树叶（特性阶段是否落块的廉价信号）。
    ///
    /// Whether the chunk palette contains log/leaves — a cheap signal that the
    /// feature stage placed blocks.
    wood: bool,
    /// 仅 `--verify` 时保留区块本体用于哈希；否则为 `None`，避免额外复制。
    ///
    /// Chunk kept for hashing only under `--verify`; otherwise `None` to avoid
    /// an extra copy.
    chunk: Option<Chunk>,
}

/// 区块调色板中是否包含原木/树叶（只扫调色板，不逐方块扫描）。
///
/// Whether a chunk's palettes contain log/leaves (palette scan only).
fn has_wood(chunk: &Chunk, wood: &[u32; 2]) -> bool {
    chunk.subchunks.iter().any(|section| {
        section.layers.iter().any(|layer| match layer {
            PalettedBlockStorage::Single(id) => wood.contains(&id.0),
            PalettedBlockStorage::Paletted { palette, .. } => {
                palette.iter().any(|id| wood.contains(&id.0))
            }
        })
    })
}

/// 逐区块摘要的初始值（FNV-1a 64 offset basis，与 `sc_worldgen::fnv1a64` 一致）。
///
/// Initial value of a per-chunk digest (FNV-1a 64 offset basis, matching
/// `sc_worldgen::fnv1a64`).
const CHUNK_HASH_SEED: u64 = 0xcbf2_9ce4_8422_2325;

fn mix(mut hash: u64, value: u64) -> u64 {
    hash ^= value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    hash = hash.rotate_left(27);
    hash.wrapping_mul(0x100_0000_01b3)
}

fn chunk_digest(mut hash: u64, chunk: &Chunk) -> u64 {
    hash = mix(hash, chunk.subchunks.len() as u64);
    for section in &chunk.subchunks {
        hash = mix(hash, u64::from(section.index.y as u8));
        for layer in &section.layers {
            match layer {
                PalettedBlockStorage::Single(id) => {
                    hash = mix(hash, 0x51);
                    hash = mix(hash, u64::from(id.0));
                }
                PalettedBlockStorage::Paletted { palette, words } => {
                    hash = mix(hash, 0x9a);
                    hash = mix(hash, palette.len() as u64);
                    for id in palette {
                        hash = mix(hash, u64::from(id.0));
                    }
                    for word in words.iter() {
                        hash = mix(hash, u64::from(*word));
                    }
                }
            }
        }
    }
    hash
}

// ---------------------------------------------------------------------------
// Coordinates
// ---------------------------------------------------------------------------

/// 从原点起的方形螺旋区块坐标（互不重复，顺序确定）。
///
/// Square spiral of distinct chunk coordinates from the origin, in a
/// deterministic order.
fn spiral_coords(count: usize, origin: (i32, i32)) -> Vec<(i32, i32)> {
    let mut out = Vec::with_capacity(count);
    if count == 0 {
        return out;
    }
    out.push((0, 0));
    let mut ring = 1i32;
    while out.len() < count {
        for x in (-ring + 1)..=ring {
            out.push((x, -ring));
        }
        for z in (-ring + 1)..=ring {
            out.push((ring, z));
        }
        for x in (-ring..=(ring - 1)).rev() {
            out.push((x, ring));
        }
        for z in (-ring + 1..=(ring - 1)).rev() {
            out.push((-ring, z));
        }
        ring += 1;
    }
    out.truncate(count);
    out.into_iter()
        .map(|(x, z)| (origin.0 + x, origin.1 + z))
        .collect()
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

/// 一轮（所有线程）的实测结果。
///
/// Result of one round (all threads).
struct RoundStats {
    elapsed: Duration,
    chunks: usize,
    digest: u64,
    spillover: usize,
    sections: usize,
    wood_chunks: usize,
}

/// 单个工作线程的累计观测值。
///
/// Accumulated observations from one worker thread.
#[derive(Default)]
struct WorkerOut {
    digest: u64,
    spillover: usize,
    sections: usize,
    wood_chunks: usize,
    chunks: usize,
}

/// 一条腿（固定线程数）的聚合结果。
///
/// Aggregated result for one leg (fixed thread count).
struct LegResult {
    threads: usize,
    rounds: Vec<RoundStats>,
    total_chunks: usize,
    total_elapsed: Duration,
    digest: u64,
    spillover: usize,
    sections: usize,
    wood_chunks: usize,
}

impl LegResult {
    /// 聚合吞吐：总区块数 / 总耗时（比逐轮 CPS 的算术平均更稳健）。
    ///
    /// Aggregate throughput: total chunks / total elapsed (more robust than the
    /// arithmetic mean of per-round CPS).
    fn cps(&self) -> f64 {
        if self.total_elapsed.is_zero() {
            return f64::NAN;
        }
        self.total_chunks as f64 / self.total_elapsed.as_secs_f64()
    }

    fn ms_per_chunk(&self) -> f64 {
        self.total_elapsed.as_secs_f64() * 1000.0 / self.total_chunks.max(1) as f64
    }

    fn round_cps(round: &RoundStats) -> f64 {
        if round.elapsed.is_zero() {
            return f64::NAN;
        }
        round.chunks as f64 / round.elapsed.as_secs_f64()
    }
}

/// 用 `threads` 个线程生成 `coords` 覆盖的全部位置，返回一轮统计。
///
/// Generates every position in `coords` with `threads` threads, returning one
/// round of statistics.
///
/// 计时从所有工作线程进入就绪屏障之后开始，因此不包含线程启动开销。
///
/// Timing starts after every worker reaches the ready barrier, so thread spawn
/// cost is excluded.
fn run_round(
    generator: &Arc<BenchGenerator>,
    coords: &[(i32, i32)],
    threads: usize,
    verify: bool,
) -> Result<RoundStats, String> {
    let ready = std::sync::Barrier::new(threads + 1);
    let start = std::sync::Barrier::new(threads + 1);
    let chunk_count = coords.len();

    std::thread::scope(|scope| -> Result<RoundStats, String> {
        let mut handles = Vec::with_capacity(threads);
        for worker in 0..threads {
            let generator = Arc::clone(generator);
            let ready = &ready;
            let start = &start;
            handles.push(scope.spawn(move || -> Result<WorkerOut, String> {
                ready.wait();
                start.wait();
                let mut out = WorkerOut::default();
                for index in (worker..chunk_count).step_by(threads) {
                    let (cx, cz) = coords[index];
                    let observed = generator.generate(cx, cz, verify)?;
                    out.chunks += 1;
                    out.sections += observed.sections;
                    out.spillover += observed.spillover;
                    out.wood_chunks += usize::from(observed.wood);
                    if verify {
                        let mut chunk_hash = CHUNK_HASH_SEED;
                        if let Some(chunk) = observed.chunk.as_ref() {
                            chunk_hash = chunk_digest(chunk_hash, chunk);
                        }
                        chunk_hash = mix(chunk_hash, observed.sections as u64);
                        chunk_hash = mix(chunk_hash, observed.spillover as u64);
                        // 逐区块摘要按加法汇总：与线程划分和完成顺序无关，
                        // 因此单线程与多线程的结果可直接比较。
                        //
                        // Per-chunk digests are summed: independent of thread
                        // partitioning and completion order, so single- and
                        // multi-threaded runs stay directly comparable.
                        out.digest = out.digest.wrapping_add(chunk_hash);
                    }
                }
                Ok(out)
            }));
        }

        ready.wait();
        let began = Instant::now();
        start.wait();

        let mut digest = 0u64;
        let mut spillover = 0usize;
        let mut sections = 0usize;
        let mut wood_chunks = 0usize;
        let mut produced = 0usize;
        for handle in handles {
            let worker = handle
                .join()
                .map_err(|_| "generation worker panicked".to_string())??;
            digest = digest.wrapping_add(worker.digest);
            spillover += worker.spillover;
            sections += worker.sections;
            wood_chunks += worker.wood_chunks;
            produced += worker.chunks;
        }
        let elapsed = began.elapsed();

        if produced != chunk_count {
            return Err(format!(
                "generated {produced}/{chunk_count} chunk positions"
            ));
        }

        Ok(RoundStats {
            elapsed,
            chunks: chunk_count,
            digest,
            spillover,
            sections,
            wood_chunks,
        })
    })
}

fn run_leg(
    config: &Config,
    generator: &Arc<BenchGenerator>,
    coords: &[(i32, i32)],
    threads: usize,
) -> Result<LegResult, String> {
    for _ in 0..config.warmup_rounds {
        run_round(generator, coords, threads, config.verify)?;
    }

    let mut rounds = Vec::with_capacity(config.rounds);
    for round in 0..config.rounds {
        let stats = run_round(generator, coords, threads, config.verify)?;
        if !config.json {
            println!(
                "  round {}/{}: {:>4} chunks in {:>9.2} ms -> {:>8.2} CPS ({:>7.2} ms/chunk)",
                round + 1,
                config.rounds,
                stats.chunks,
                stats.elapsed.as_secs_f64() * 1000.0,
                LegResult::round_cps(&stats),
                stats.elapsed.as_secs_f64() * 1000.0 / stats.chunks.max(1) as f64,
            );
        }
        rounds.push(stats);
    }

    let total_chunks: usize = rounds.iter().map(|round| round.chunks).sum();
    let total_elapsed: Duration = rounds.iter().map(|round| round.elapsed).sum();
    let mut digest = 0u64;
    let mut spillover = 0usize;
    let mut sections = 0usize;
    let mut wood_chunks = 0usize;
    for round in &rounds {
        digest = mix(digest, round.digest);
        spillover += round.spillover;
        sections += round.sections;
        wood_chunks += round.wood_chunks;
    }

    Ok(LegResult {
        threads,
        rounds,
        total_chunks,
        total_elapsed,
        digest,
        spillover,
        sections,
        wood_chunks,
    })
}

// ---------------------------------------------------------------------------
// Non-flat evidence (--probe)
// ---------------------------------------------------------------------------

/// 链接后路由根的节点种类普查：用来区分「真实密度函数图」与「常数/平坦计划」。
///
/// Node-kind census over the linked router roots: distinguishes a real density
/// graph from a constant (flat) plan.
#[derive(Default)]
struct PlanCensus {
    constant: usize,
    coord: usize,
    noise: usize,
    blended: usize,
    spline: usize,
    other: usize,
}

impl PlanCensus {
    fn of(compiled: &CompiledWorldgen) -> Self {
        let mut census = Self::default();
        for node in compiled.settings().router.values() {
            node.walk(&mut |node| match node {
                Node::Constant(_) => census.constant += 1,
                Node::Coord(_) => census.coord += 1,
                Node::Noise { .. } => census.noise += 1,
                Node::OldBlendedNoise { .. } => census.blended += 1,
                Node::Spline(_) => census.spline += 1,
                _ => census.other += 1,
            });
        }
        census
    }

    fn total(&self) -> usize {
        self.constant + self.coord + self.noise + self.blended + self.spline + self.other
    }

    /// 真实地形至少需要噪声/混合噪声与坐标节点；只有常数就是 flat。
    ///
    /// Real terrain needs noise / blended noise and coordinate nodes; constants
    /// alone mean a flat plan.
    fn looks_flat(&self) -> bool {
        self.noise + self.blended == 0 || self.total() < 8
    }
}

fn block_name(runtime_id: u32) -> String {
    sc_world::block_dictionary::BlockStateDictionary::global()
        .get(runtime_id)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| format!("hash:0x{runtime_id:08x}"))
}

/// 生成 `count` 个区块并打印「这确实是真实地形」的证据。
///
/// Generates `count` chunks and prints evidence that this is real terrain.
fn run_probe(
    config: &Config,
    generator: &Arc<BenchGenerator>,
    compiled: &CompiledWorldgen,
    coords: &[(i32, i32)],
) -> Result<(), String> {
    use std::collections::BTreeMap;

    let census = PlanCensus::of(compiled);
    let probe_chunks = coords.len().min(64);
    let air = sc_world::block_dictionary::air_runtime_id();
    let sea_water = SurfaceBlockPalette::default().water.0;

    let mut heights: Vec<i32> = Vec::with_capacity(probe_chunks * 256);
    let mut surface_histogram: BTreeMap<String, usize> = BTreeMap::new();
    let mut block_histogram: BTreeMap<u32, u64> = BTreeMap::new();
    let mut water_at_sea_level = 0usize;
    let mut profile: Option<(String, i32, i32)> = None;

    for (index, &(cx, cz)) in coords.iter().take(probe_chunks).enumerate() {
        let observed = generator.generate(cx, cz, true)?;
        let Some(chunk) = observed.chunk else {
            return Err("probe requested a chunk but none was returned".to_string());
        };

        for lx in 0u8..16 {
            for lz in 0u8..16 {
                let mut top: Option<(i32, u32)> = None;
                for y in (chunk.min_y..=chunk.max_y).rev() {
                    if let Some(block) = chunk.block_at(lx, y, lz) {
                        if block.0 != air {
                            top = Some((y, block.0));
                            break;
                        }
                    }
                }
                if let Some((y, id)) = top {
                    heights.push(y);
                    *surface_histogram.entry(block_name(id)).or_default() += 1;
                    if id == sea_water && y == 63 {
                        water_at_sea_level += 1;
                    }
                }
            }
        }

        // 逐方块直方图（只统计非空气；探针区块数受限，成本可接受）。
        //
        // Per-block histogram (non-air only; probe chunk count is bounded).
        for section in &chunk.subchunks {
            for layer in &section.layers {
                for slot in 0..4096usize {
                    if let Some(block) = layer.get(slot) {
                        if block.0 != air {
                            *block_histogram.entry(block.0).or_default() += 1;
                        }
                    }
                }
            }
        }

        // 第一条区块的一列剖面（尽量取地形中部，避开树）。
        //
        // One column profile from the first chunk.
        if index == 0 && profile.is_none() {
            let (lx, lz) = (8u8, 8u8);
            let mut runs: Vec<String> = Vec::new();
            let mut run_start = chunk.min_y;
            let mut current: Option<u32> = chunk.block_at(lx, chunk.min_y, lz).map(|b| b.0);
            for y in (chunk.min_y + 1)..=chunk.max_y {
                let here = chunk.block_at(lx, y, lz).map(|b| b.0);
                if here != current {
                    if let Some(id) = current {
                        runs.push(format!(
                            "y={run_start}..{} {} ({} blocks)",
                            y - 1,
                            block_name(id),
                            y - run_start
                        ));
                    }
                    run_start = y;
                    current = here;
                }
            }
            if let Some(id) = current {
                runs.push(format!(
                    "y={run_start}..{} {} ({} blocks)",
                    chunk.max_y,
                    block_name(id),
                    chunk.max_y - run_start + 1
                ));
            }
            profile = Some((runs.join("\n      "), chunk.min_y, chunk.max_y));
        }
    }

    if heights.is_empty() {
        return Err("probe found no solid columns".to_string());
    }

    let mut distinct: Vec<i32> = heights.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let min = *distinct.first().unwrap();
    let max = *distinct.last().unwrap();
    let mean = heights.iter().map(|h| f64::from(*h)).sum::<f64>() / heights.len() as f64;
    let columns = heights.len();

    println!("probe: {probe_chunks} chunks (single-threaded) from spiral at ({}, {})", config.origin.0, config.origin.1);
    println!(
        "  linked plan census: {} nodes — constant {}, coord {}, noise {}, old_blended_noise {}, \
         spline {}, other {}",
        census.total(),
        census.constant,
        census.coord,
        census.noise,
        census.blended,
        census.spline,
        census.other
    );
    println!(
        "  runtime samples: final_density @ the 1225 cell corners of every chunk \
         ({} router roots, {} noise tables, {} plan nodes)",
        compiled.settings().router.len(),
        compiled.settings().noises.len(),
        compiled.plan().node_count()
    );
    println!(
        "  surface heights (topmost non-air block per column): min {min}, max {max}, \
         mean {mean:.1}, {} distinct value(s) over {columns} columns",
        distinct.len()
    );
    println!("  columns topped by water at Y=63: {water_at_sea_level}/{columns}");
    println!("  surface block histogram:");
    let mut surface: Vec<(&String, &usize)> = surface_histogram.iter().collect();
    surface.sort_by(|a, b| b.1.cmp(a.1));
    for (name, count) in surface.iter().take(8) {
        println!("    {name:<28} {count}");
    }
    println!("  non-air block histogram (top 10):");
    let mut blocks: Vec<(&u32, &u64)> = block_histogram.iter().collect();
    blocks.sort_by(|a, b| b.1.cmp(a.1));
    let total_blocks: u64 = block_histogram.values().sum();
    for (id, count) in blocks.iter().take(10) {
        let share = 100.0 * **count as f64 / total_blocks.max(1) as f64;
        println!(
            "    {:<28} {count:>12}  ({share:>5.2}%)",
            block_name(**id)
        );
    }
    if let Some((runs, min_y, max_y)) = &profile {
        println!("  column profile (chunk local 8,8; y={min_y}..{max_y}):");
        println!("      {runs}");
    }

    if census.looks_flat() {
        println!(
            "  VERDICT: FLAT/SYNTHETIC — the linked plan has {} noise nodes and {} nodes total",
            census.noise + census.blended,
            census.total()
        );
    } else {
        println!(
            "  VERDICT: real density-function terrain — {} noise/blended nodes, \
             {} distinct surface heights, {} block types",
            census.noise + census.blended,
            distinct.len(),
            block_histogram.len()
        );
    }
    if distinct.len() < 3 {
        return Err(format!(
            "probe saw only {} distinct surface height(s): terrain is effectively flat",
            distinct.len()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return;
    }
    let config = match Config::parse(args.into_iter()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("worldgen_cps: {err}\n");
            print_usage();
            std::process::exit(2);
        }
    };
    if let Err(err) = run(&config) {
        eprintln!("worldgen_cps: {err}");
        std::process::exit(1);
    }
}

fn run(config: &Config) -> Result<(), String> {
    let pack = find_version_pack(config.pack.as_deref())?;

    let started = Instant::now();
    let sources = load_sources(&pack)?;
    let load_elapsed = started.elapsed();

    let started = Instant::now();
    let compiled = CompiledWorldgen::compile(&sources, "minecraft:overworld")
        .map_err(|err| format!("CompiledWorldgen::compile failed: {err}"))?;
    let compile_elapsed = started.elapsed();

    let generator = Arc::new(match config.stage {
        Stage::Full => BenchGenerator::new_full(&compiled, config.seed),
        Stage::Base => BenchGenerator::new_base(&compiled, config.seed),
    });

    let coords = spiral_coords(config.chunks, config.origin);
    let census = PlanCensus::of(&compiled);
    let logical_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);

    if !config.json {
        println!("sc_worldgen CPS bench");
        println!("  pack:          {}", pack.display());
        println!("  seed:          {}", config.seed);
        println!("  stage:         {} ({})", config.stage.label(), stage_entry(config.stage));
        println!(
            "  shape:         {} chunks/round x {} rounds (+{} warmup)",
            config.chunks, config.rounds, config.warmup_rounds
        );
        println!(
            "  coords:        spiral from ({}, {})",
            config.origin.0, config.origin.1
        );
        println!(
            "  sources:       {} files loaded in {:.2} ms",
            sources.len(),
            load_elapsed.as_secs_f64() * 1000.0
        );
        println!(
            "  compile:       {} ms — {} router roots, {} noise tables, {} plan nodes",
            compile_elapsed.as_millis(),
            compiled.settings().router.len(),
            compiled.settings().noises.len(),
            compiled.plan().node_count()
        );
        println!(
            "  plan census:   {} linked nodes — constant {}, coord {}, noise {}, \
             old_blended_noise {}, spline {}, other {}",
            census.total(),
            census.constant,
            census.coord,
            census.noise,
            census.blended,
            census.spline,
            census.other
        );
        println!(
            "  cpu:           {} logical cores, measuring {:?}",
            logical_cores, config.threads
        );
        if config.verify {
            println!(
                "  verify:        on — every chunk is hashed (numbers include hashing cost)"
            );
        }
        if config.threads.len() > 1 {
            println!(
                "  order:         legs run in the listed order — run `--threads {}` to check \
                 thermal/ordering drift",
                config
                    .threads
                    .iter()
                    .rev()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        println!();
    }

    if config.probe {
        if !config.json {
            println!();
        }
        run_probe(config, &generator, &compiled, &coords)?;
        if !config.json {
            println!();
        }
        if config.probe_only {
            return Ok(());
        }
    }

    let mut legs: Vec<LegResult> = Vec::with_capacity(config.threads.len());
    let mut filtered_out = 0usize;
    let mut failure: Option<String> = None;
    emit_suite_event(config, r#"{"type":"suite","event":"started","name":"worldgen_cps"}"#);
    for &threads in &config.threads {
        let name = leg_name(config.stage, threads);
        if !leg_selected(config, &name) {
            filtered_out += 1;
            continue;
        }
        if !config.json {
            println!("[{name}]");
        }
        emit_suite_event(
            config,
            &format!(
                r#"{{"type":"test","event":"started","name":"{name}","bench":true}}"#
            ),
        );
        match run_leg(config, &generator, &coords, threads) {
            Ok(leg) => {
                if !config.json {
                    println!(
                        "  aggregate: {} chunks over {} rounds / {:.2} s -> {:.2} CPS \
                         ({:.2} ms/chunk)\n",
                        leg.total_chunks,
                        leg.rounds.len(),
                        leg.total_elapsed.as_secs_f64(),
                        leg.cps(),
                        leg.ms_per_chunk(),
                    );
                }
                emit_suite_event(
                    config,
                    &format!(
                        r#"{{"type":"test","event":"ok","name":"{name}","exec_time":{:.3}}}"#,
                        leg.total_elapsed.as_secs_f64(),
                    ),
                );
                legs.push(leg);
            }
            Err(err) => {
                let message = format!("{name}: {err}");
                emit_suite_event(
                    config,
                    &format!(
                        r#"{{"type":"test","event":"failed","name":"{name}","failure-output":"final:{}"}}"#,
                        json_escape(&message),
                    ),
                );
                failure = Some(message);
                break;
            }
        }
    }

    if filtered_out > 0 && !config.json {
        let names: Vec<String> = config
            .threads
            .iter()
            .map(|&threads| leg_name(config.stage, threads))
            .collect();
        println!(
            "note: {filtered_out} leg(s) filtered out — available legs: {}",
            names.join(", ")
        );
    }
    if legs.is_empty() && failure.is_none() {
        eprintln!(
            "worldgen_cps: no leg matched the filter {:?}; available legs: {:?}",
            config.filters,
            config
                .threads
                .iter()
                .map(|&threads| leg_name(config.stage, threads))
                .collect::<Vec<_>>()
        );
        emit_suite_event(
            config,
            &format!(
                r#"{{"type":"suite","event":"ok","executed":0,"failed":0,"filtered-out":{},"ignored":0,"result":"ok"}}"#,
                filtered_out
            ),
        );
        return Ok(());
    }

    let baseline = legs
        .iter()
        .find(|leg| leg.threads == 1)
        .map(|leg| leg.cps())
        .unwrap_or(f64::NAN);
    let mut verify_problem = None;
    if config.verify {
        let reference = legs.iter().find(|leg| leg.threads == 1);
        if let Some(reference) = reference {
            for leg in &legs {
                if leg.threads != 1 && leg.digest != reference.digest {
                    verify_problem = Some(format!(
                        "threads={} digest {:016x} != single-thread digest {:016x}",
                        leg.threads, leg.digest, reference.digest
                    ));
                    break;
                }
            }
        }
    }

    if config.json {
        print_json(config, &pack, &compiled, &legs, baseline, verify_problem.as_deref());
    } else {
        print_summary(&legs, baseline, &compiled);
        if let Some(problem) = &verify_problem {
            println!("VERIFY FAILED: {problem}");
        } else if config.verify {
            println!("verify: single-thread and multi-thread digests match");
        }
    }

    if let Some(message) = &failure {
        emit_suite_event(
            config,
            &format!(
                r#"{{"type":"suite","event":"failed","executed":{},"failed":1,"filtered-out":{},"ignored":0,"result":"failed","failures":["{}"]}}"#,
                legs.len(),
                filtered_out,
                json_escape(message),
            ),
        );
        return Err(message.clone());
    }
    if verify_problem.is_some() {
        emit_suite_event(
            config,
            &format!(
                r#"{{"type":"suite","event":"failed","executed":{},"failed":1,"filtered-out":{},"ignored":0,"result":"failed","failures":["verify"]}}"#,
                legs.len(),
                filtered_out
            ),
        );
        return Err("multi-threaded generation produced a different result set".to_string());
    }
    emit_suite_event(
        config,
        &format!(
            r#"{{"type":"suite","event":"ok","executed":{},"failed":0,"filtered-out":{},"ignored":0,"result":"ok"}}"#,
            legs.len(),
            filtered_out
        ),
    );
    Ok(())
}

/// leg 名：`<stage>/threads=<n>`（同时是 IDE 里可重跑的用例名）。
///
/// Leg name: `<stage>/threads=<n>` — also the case name an IDE can re-run.
fn leg_name(stage: Stage, threads: usize) -> String {
    format!("{}/threads={}", stage.label(), threads)
}

/// 位置参数 / `--skip` 过滤：子串匹配，`--exact` 时要求全等。
///
/// Positional / `--skip` filtering: substring match, equality with `--exact`.
fn leg_selected(config: &Config, name: &str) -> bool {
    if config.skips.iter().any(|skip| name.contains(skip.as_str())) {
        return false;
    }
    if config.filters.is_empty() {
        return true;
    }
    config.filters.iter().any(|filter| {
        if config.exact {
            name == filter
        } else {
            name.contains(filter.as_str())
        }
    })
}

/// 仅当 IDE 请求 `--format=json`（且未请求本基准自己的 `--json`）时输出事件行。
///
/// Emits a libtest-shaped event line only when the IDE asked for `--format=json`
/// and this benchmark's own `--json` schema was not requested.
fn emit_suite_event(config: &Config, payload: &str) {
    if config.ide_json && !config.json {
        println!("{payload}");
    }
}

/// JSON 字符串转义（只处理错误文本可能出现的字符）。
///
/// Minimal JSON string escaping for error text.
fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", other as u32)),
            other => out.push(other),
        }
    }
    out
}

fn stage_entry(stage: Stage) -> &'static str {
    match stage {
        Stage::Full => "BuiltinWorldGenerator::generate_chunk_with_spillover_bounded",
        Stage::Base => "BaseTerrainGenerator::generate_base_chunk",
    }
}

fn print_summary(legs: &[LegResult], baseline: f64, compiled: &CompiledWorldgen) {
    println!("== summary ==");
    println!(
        "{:>8} | {:>10} | {:>10} | {:>9} | {:>10} | {:>9}",
        "threads", "CPS", "ms/chunk", "speedup", "efficiency", "chunks"
    );
    for leg in legs {
        let speedup = if baseline.is_finite() && baseline > 0.0 {
            leg.cps() / baseline
        } else {
            f64::NAN
        };
        let efficiency = speedup / leg.threads as f64 * 100.0;
        println!(
            "{:>8} | {:>10.2} | {:>10.2} | {:>8.2}x | {:>9.1}% | {:>9}",
            leg.threads,
            leg.cps(),
            leg.ms_per_chunk(),
            speedup,
            efficiency,
            leg.total_chunks,
        );
    }
    println!();
    for leg in legs {
        println!(
            "  threads={:<3} spillover writes: {:<6} tree-bearing chunks: {:<6}              sections/chunk: {:.1}",
            leg.threads,
            leg.spillover,
            leg.wood_chunks,
            leg.sections as f64 / leg.total_chunks.max(1) as f64,
        );
    }
    println!(
        "closure fingerprint {:016x}; CPS = chunks / wall-clock of the generation loop only",
        compiled.prepared().closure_fingerprint
    );
    println!(
        "boundary: no storage commit, no network encoding, no provider queueing, no lighting"
    );
}

fn print_json(
    config: &Config,
    pack: &Path,
    compiled: &CompiledWorldgen,
    legs: &[LegResult],
    baseline: f64,
    verify_problem: Option<&str>,
) {
    let legs_json: Vec<String> = legs
        .iter()
        .map(|leg| {
            let speedup = if baseline.is_finite() && baseline > 0.0 {
                leg.cps() / baseline
            } else {
                1.0
            };
            let rounds: Vec<String> = leg
                .rounds
                .iter()
                .map(|round| {
                    format!(
                        "{{\"chunks\":{},\"elapsed_ms\":{:.3},\"cps\":{:.3}}}",
                        round.chunks,
                        round.elapsed.as_secs_f64() * 1000.0,
                        LegResult::round_cps(round)
                    )
                })
                .collect();
            format!(
                "{{\"threads\":{},\"chunks\":{},\"elapsed_ms\":{:.3},\"cps\":{:.3},\
                 \"ms_per_chunk\":{:.3},\"speedup\":{:.4},\"efficiency\":{:.4},\
                 \"spillover_writes\":{},\"tree_bearing_chunks\":{},\"sections\":{},\
                 \"digest\":\"{:016x}\",\"rounds\":[{}]}}",
                leg.threads,
                leg.total_chunks,
                leg.total_elapsed.as_secs_f64() * 1000.0,
                leg.cps(),
                leg.ms_per_chunk(),
                speedup,
                speedup / leg.threads as f64,
                leg.spillover,
                leg.wood_chunks,
                leg.sections,
                leg.digest,
                rounds.join(","),
            )
        })
        .collect();

    println!(
        "{{\"pack\":\"{}\",\"seed\":{},\"stage\":\"{}\",\"chunks_per_round\":{},\
         \"rounds\":{},\"warmup_rounds\":{},\"origin\":[{},{}],\"verify\":{},\
         \"plan_nodes\":{},\"router_roots\":{},\"noise_tables\":{},\
         \"closure_fingerprint\":\"{:016x}\",\"legs\":[{}],\"verify_error\":{}}}",
        pack.display().to_string().replace('\\', "/").replace('"', "\\\""),
        config.seed,
        config.stage.label(),
        config.chunks,
        config.rounds,
        config.warmup_rounds,
        config.origin.0,
        config.origin.1,
        config.verify,
        compiled.plan().node_count(),
        compiled.settings().router.len(),
        compiled.settings().noises.len(),
        compiled.prepared().closure_fingerprint,
        legs_json.join(","),
        match verify_problem {
            Some(problem) => format!("\"{}\"", problem.replace('"', "\\\"")),
            None => "null".to_string(),
        }
    );
}
