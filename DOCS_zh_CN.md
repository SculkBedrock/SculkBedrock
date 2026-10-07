# SculkBedrock — 开发者指南（DOCS）

> 语言：[English](DOCS.md) | 简体中文
>
> [README_zh_CN.md](README_zh_CN.md) 的开发者配套文档

## 0. 目录

1. [前置条件与仓库结构](#1-前置条件与仓库结构)
2. [构建、测试、运行](#2-构建测试运行)
3. [启动管线](#3-启动管线)
4. [版本包管线](#4-版本包管线)
5. [方块 / 物品 / 配方数据管线](#5-方块--物品--配方数据管线)
6. [区块管线](#6-区块管线)
7. [网络与协议管线](#7-网络与协议管线)
8. [世界生成插件管线](#8-世界生成插件管线)
9. [插件开发](#9-插件开发)
10. [持久化管线](#10-持久化管线)
11. [验证与诊断](#11-验证与诊断)
12. [已实现与路线图](#12-已实现与路线图)
13. [贡献规则](#13-贡献规则)

## 1. 前置条件与仓库结构

- Stable Rust 工具链 + Cargo。
- 版本包（`.scver`）：<https://github.com/SculkBedrock/SculkBedrock-VersionPacks>，放入 `version_packs/`。包内插件源码在该仓库的 `crates/ur_vanilla` 下。
- 默认 UDP 端口 `19132`；配置 `server_properties.toml`；存档 `worlds/`；日志 `logs/`。

```text
crates/
  sc_bootstrap/   入口（sc_bootstrap::run）+ 启动系统
  sc_ecs/         自研 ECS（World、资源、调度、区域）
  sc_game/        玩法（区域、交互、意图、配方视图）
  sc_world/       区块、执行器、LevelDB、写回、字典
  sc_network/     协议编解码、连接、区块管线、hook
  sc_raknet/      RakNet 传输、重传恢复队列
  sc_block/       方块注册表 + 编译后的方块快照
  sc_item/        物品注册表 + 库存
  sc_recipe/      配方快照编译器
  sc_packloader/  版本包 IO、schema、预算、诊断
  sc_plugin/      插件管理 + 加载器（rust / C-ABI）
  sc_plugin_api/  稳定 C-ABI 宿主表（HostApiV1）
  sc_vanilla/     原版生成器 / 命令插件源码
  sc_*/           二进制、NBT、工具、日志、控制台、molang、实体……
tools/            包与代码生成脚本（见 §4）
server_properties.toml / worlds/ / version_packs/ / logs/
```

## 2. 构建测试运行

仓库很大，所有命令都用受限并行：

```shell
cargo check -p sc_bootstrap -j 2
cargo run -p sc_bootstrap -j 2
cargo test -p sc_world -j 1 --lib
cargo test -p sc_network -j 1 --lib
cargo test -p sc_block -j 1 --lib
cargo test -p sc_packloader -j 1 --lib
```

约定：

- 只针对改动的 crate 做 `cargo check` / `cargo test`。
- 全量构建与真机联调很重，不要随手跑。
- 改动文件应通过 `rustfmt --check` 与 `git diff --check`。

## 3. 启动管线

入口 `crates/sc_bootstrap/src/main.rs` → `sc_bootstrap::run()`，按调度顺序执行：

```text
PreStartup:  init_log, init_properties
              （日志 + server_properties.toml → ServerProperties 资源，
                控制台语言、SCULK_LOCALE 透传给动态插件）
SCStartup:   startup
              （信号处理、文件日志、StartupTimestamp、Server::init）
SCPreLoad:   packloader load_version
              （解析 .scver → SCVersionPack 资源：manifest、runtime id、
                方块包/物品/配方/世界生成/插件……）
SCLoad:      load_block_bundle     （方块 JSON 编译 → 不可变快照，
                                      原子发布；新旧源绝不混用）
             load_block_palette    （未声明 block_data 时走旧 palette 路径）
             load_item_registry    （runtime 条目 → ItemRegistry；方块物品
                                      链接默认态哈希；air/shield 检查）
             load_recipe_registry  （行为包配方 → 不可变快照 +
                                      按注册表过滤的组表）
SCPostLoad:  load_version_pack_plugins（kind:"rust" 走内置工厂，
                                        kind:"cabi" 走 dlopen + HostApiV1）
             release_version_pack_bulk_data（释放 worldgen/tag/palette 等
                                              加载期大内存，只留 manifest）
PostStartup: finish_startup（打印启动耗时）
PostUpdate:  check_shutdown_signal（SIGINT/SIGTERM/SIGHUP → SCExit → 优雅退出）
```

停服是事件驱动（`SCExit`）：区域线程停、新任务拒收、worker 排空有界结果、脏数据按预算落盘并报告未确认写入，最后关闭 DB owner。有界等待绝不在 DB 写一半时杀线程，只报告。

## 4. 版本包管线

`.scver` 本质是 zip，大致结构：

```text
manifest.json           （包身份、协议/游戏版本、block_data 声明）
definitions/
  blocks/**/*.block.json（每种方块一文件，原版结构 + sc: 扩展）
  runtime.json          （物品名 → 网络 id）
  block_palette.nbt     （仅旧路径使用）
  block_tags.json       （包级 tag）
  recipe_groups.json    （配料组，启动时按注册表过滤）
  biomes/、entity_identifiers.nbt、worldgen/…
behavior_packs/         （配方、物品组件……）
plugins/                （*.scplugin：manifest.json + plugin.so，或 kind:"rust"）
```

关键规则：

- 声明 `block_data` 即切换到新方块路径；新旧方块源绝不混用。bundle 失败一律 loud 报错，不静默回退为空气/石头。
- `tools/inject_block_json_pack.py` 注入生成的 `.block.json` 并写 manifest 声明。
- `tools/rewrite_block_pack.py`、`tools/blockgen_*.py` 从参考源抽取/派生方块数据（默认态、硬度、掉落表）。
- `tools/gen_recipe_groups.py` 重新生成配料组表。
- `tools/package_sc_vanilla_macos.py`（与 `.ps1` 双子）重打包内 Rust DLL。
- **锁步重编规则：** 任何跨 DLL 可见资源布局变化（例如给 `SCVersionPack` 加字段）都必须同步重编包内全部 Rust DLL 插件，否则 DLL 侧拿不到资源。C-ABI 表本身不变则不受影响，改表必须升 ABI 版本。

## 5. 方块 / 物品 / 配方数据管线

```text
.block.json 文件
 → packloader schema 解析（路径、预算、重复 key 拒绝、
                             规范状态枚举、受限
                             q.block_property(...) == ... 条件）
 → 方块编译器（私有 builder → 稠密状态表：
                 碰撞、挖掘秒数、不可破坏、光照、
                 掉落引用、可替换/液体/随机刻……、挖掘/掉落 profile）
 → 原子发布（BlockJsonRegistry Arc 快照 + BlockStateRegistry +
              能力位 + 旧 palette 派生适配字节）
 → 游戏消费（挖掘 tick = ceil(秒数×20)、破坏进度增量、
              提交后掷骰掉落）
```

- 状态是声明属性值的笛卡尔积，按规范顺序枚举；`sc:protocol_runtime_ids` 一一对齐，永远来自抽取源，不重算。
- `permutations` 按命中状态整体替换组件值；同一状态重叠覆盖即整包失败。
- 物品：`runtime.json` 条目进 `ItemRegistry`；命中方块字典的即方块物品，链接默认态哈希。缺 air 即包损坏。
- 配方：行为包按包序低→高编译（同层重复即硬错误）；未知配方类型拒绝；组表只按 live 物品注册表过滤。

## 6. 区块管线

### 6.1 阶段

```text
View Planner（desired 差量、距离 + 年龄优先级、view/context 票据）
  → Chunk Coordinator（按世界实例 + key 去重、预算、作业状态）
  → Chunk Executor（有界 IO + 生成 worker → 私有结果）
  → 属主发布（身份校验 → 权威列）
  → Encode Coordinator（按列身份 + wire profile 共享编码）
  → Delivery Pump / ConnectionGate（hook、屏障、tick/字节准入）
  → 玩家 used/spawn 进度（仅 Queued 回执推进）
Writeback Coordinator 独立运行（快照 → DB owner → SaveAck）。
```

三个窗口互相独立：**desired**（目标 key）、**prepare**（有界的已批准加载/编码需求）、**delivery**（等配额的小批量 ready）。视距 10 绝不能一次提交数百个任务。

### 6.2 身份与状态

至少区分：世界实例、连接会话、context epoch（传送/换维度屏障）、view revision（普通移动保留重叠）、列 incarnation + 内容 generation、任务 attempt、wire profile。payload 缓存身份 = 世界 + 维度/坐标 + incarnation + generation + profile。

列生命周期：`Absent → Queued → Loading/Generating → Applying → Ready`，另有 `Failed/Backoff` 与 `Closing`。单玩家单 key 进度：`Wanted → AwaitingColumn → AwaitingPayload → Preparing → ReadyToDeliver → Admitted → SentForContext`，另有释放/取消/失败。加载就绪、编码就绪、网络准入、客户端初始化是四种不同事实。

### 6.3 贡献者须知

- 同世界同 key 需求合并；缓存命中绕过 worker。
- 已接受作业必有终态：成功/失败/取消/worker 故障/关闭；panic 被隔离。
- Spillover 按操作 ID 全序仲裁（不看 worker 到达顺序），并校验目标列权威实例。
- 基线进有序连接链之前必须过内容/context/预算终检；缺口回退整列重编码，不做无界 delta。
- 遵守预算：每玩家每 tick 有效准入（默认 4）、出生阈值对照可达集合校验（默认 56）、准备窗口（默认 12）、重传恢复队列的条数 + 字节 + 年龄上限、脏水位暂停新生成。

## 7. 网络与协议管线

```text
Bedrock 包 → 解码（按连接 ProtocolProfile）
 → 边界请求（协议无关）
 → 属主路由 → 校验 + 插件决策 → 提交
 → 权威事实 + 回执 → NetworkIntent
 → 意图 hook → 包翻译 → 包 hook
 → ConnectionGate（有序、有预算）→ RakNet（可靠）
```

- 两级 hook：**意图 hook**（游戏语义，可撤回）与**包 hook**（协议语义：检查/反作弊/审计）。结果区分 `Admitted / Cancelled / SuppressedByDebug / Busy / StaleContext / Closed / Failed`。跨 hook `await` 不持 ECS/区块/视图锁。
- `Admitted` 只表示进入可靠有序链，不等于 UDP 已发，更不等于客户端已渲染。
- 玩家生命周期：连接 → 登录序列 → 出生流（创造背包、库存、出生阈值）→ `InGame` → 移动/交互意图 → 传送/重置屏障 → 断线（列表/移除广播、租约清理）。
- 新增数据包：补 codec + profile 映射 + golden 用例；不改布局就不动 profile 和用例。

## 8. 世界生成插件管线

```text
包 worldgen + 种子 + 生成器描述
 → 原版生成器插件校验/编译不可变运行时
 → worker 上跑有界生成任务
 → 属主校验身份 → 发布 ChunkColumn
```

- C-ABI 生成器经 `HostApiV1::register_world_generator(world_kind, generate)` 注册，收到 `ChunkGenRequest { x, z, dimension, min_y, max_y }`，经 `set_block_cb` 回调写入在建区块。
- 生成用方块哈希走规范 `name+states` FNV1a-32 合同。
- 存档记录生成器身份；打开旧存档沿用原描述，绝不静默改变已有存档地形。

## 9. 插件开发

| Kind | 打包 | 执行 |
|---|---|---|
| `rust` | 包 `plugins/` 下 manifest 占位；代码随服务端编译，按名匹配内置工厂 | 进程内，可直接用 ECS，必须走属主命令 |
| `cabi`（默认） | `*.scplugin` 内含 `manifest.json` + `plugin.so/.dll/.dylib` | dlopen，只允许 `sc_plugin_api`（`HostApiV1`：生成器、方块哈希、日志） |

生命周期：依赖检查 → 启用 → 注册系统/生成器 → 禁用/卸载时先拒新回调、注销系统、取消/排空任务，最后销毁上下文/卸载动态库。宿主表布局 breaking 变化必须升 major/协商 ABI，绝不静默扩展 V1。

## 10. 持久化管线

```text
属主快照(g) → 释放锁 → 有界 WritebackQueue
 → DB owner 线程落盘 → SaveAck(key, g, 结果)
 → 仅当无更新时清除 ≤ g 的 dirty
```

- 每个 LevelDB 实例一个 owner 线程；游戏线程绝不直碰 DB 对象。
- 旧快照不覆盖新版本；失败保留 dirty 有界退避。
- Spillover 是玩法事实，按有界 journal 心态处理：稳定操作 ID、去重、重放、目标列应用、清理点，绝不静默丢。
- 卸载/停服顺序：停新准入 → 取消无消费者视觉任务 → 排空 completion → 结束事务/journal → 按 key 检查 `SaveAck` 的预算落盘 → 关闭 DB owner → 允许重开（新实例 ID）。

## 11. 验证与诊断

```shell
cargo test -p sc_world -j 1 --lib        # 存储、执行器、外溢、写回
cargo test -p sc_network -j 1 --lib      # 编码器、交付屏障、预算
cargo test -p sc_block -j 1 --lib        # 快照、真实包等价
cargo test -p sc_packloader -j 1 --lib   # schema、条件、预算
cargo test -p sc_game -j 1 --lib         # 交互、出站准入
```

- 真实包测试编译全量方块并与旧 palette 逐状态比对；挖掘覆盖度在启动日志报告（已声明 / 不可破坏 / 回退数）。
- 场景覆盖：外溢确定性、context/传送屏障、重叠保留移动、共享需求去重、世界隔离、重开 fencing、有界队列终态、worker 故障、hook 取消语义、编码/保存竞态、出生阈值诊断、重传预算、慢连接处理、dirty/背压、有界停服、重启 journal、登录流、未知 NBT 存档往返、种子/顺序确定性。
- 单元测试替代不了的（需真机/真盘/断电）：RSS/分配器/吞吐基线、完整登录→区块→传送、慢客户端/慢盘、崩溃/断电恢复。
- 观测：`logs/`、区域 tick 统计、队列字节/最老年龄、事务 accept/prepare/commit/abort 计数、desired/prepare/ready/admitted 分布、按连接准入字节、hook 延迟、dirty 字节/年龄、`SaveAck` 版本、分配器/RSS。

## 12. 状态选择框

所有 `[x]` 均为部分可用，不是生产验收。

### 已完成

**核心框架**

- [x] 自研 ECS（世界、资源、调度）
- [x] 启动管线（配置 → 版本包 → 注册表 → 插件 → 监听）
- [x] 版本包加载，schema 严格校验、失败 loud 报错

**方块**

- [x] 单方块 JSON 定义，规范状态枚举与原子发布
- [x] 稠密能力表（碰撞、光照、可替换、随机刻过滤……）
- [x] 服务端权威挖掘计时（`ceil(秒数×20)`，生存模式不可破坏拒绝）

**区块**

- [x] 视图跟踪，普通移动保留重叠、硬 context 重置
- [x] 共享需求合并（同世界 + key）与有界执行器
- [x] 按内容身份与 wire profile 的共享编码
- [x] 按操作 ID 的确定性外溢仲裁

**网络**

- [x] 按连接有序交付与显式回执（仅 `Queued` 推进进度）
- [x] 出站两级准入（可覆盖状态 vs 可靠事实）
- [x] 意图 hook + 包 hook，传送/换维度 `StaleContext` 屏障
- [x] RakNet 重传预算；恢复队列条数 + 字节 + 年龄上限

**持久化**

- [x] LevelDB 单 owner 线程与后台有界写回
- [x] 版本化 `SaveAck` 确认；失败保留 dirty
- [x] 有界停服与未确认写入报告

**安全护栏**

- [x] 有界日志队列、解压输出上限、超大 NBT 拒绝

### 未完成

**红石系统**

- [ ] 红石粉能量传播
- [ ] 中继器、比较器与时序行为
- [ ] 活塞（含跨区块 / 跨区域推动）
- [ ] 红石驱动的多格跨区域协调
- [ ] 红石元件的随机刻调度

**地图生成**

- [ ] 版本包数据驱动的密度函数 / 噪声路由管线
- [ ] 群系选择与地表规则端到端接线
- [ ] 生成器身份写入存档（指纹接线）
- [ ] 同种子 + 同 profile ⇒ 同地形保证
- [ ] 结构 / 树木放置与跨重启安全 journal

**方块**

- [ ] BlockEntity 落地（坐标索引、幂等装配、生命周期卸载）
- [ ] 特殊方块 `behaviors` 绑定（门、床、容器……）
- [ ] 外部战利品表解析
- [ ] 按工具挖掘规则、收获门控、时运表
- [ ] 精准采集替换、物品 tag、耐久、经验掉落

**物品与库存**

- [x] 物品注册表（含方块物品映射）与创造背包填充
- [ ] 库存预约与跨属主事务
- [ ] 完整容器事务（含回滚与重同步）
- [ ] 耐久 / 无限耐久消耗
- [ ] 冒险模式 `CanDestroy` 名单

**实体**

- [x] 基础移动、物理与广播
- [ ] 实体生命周期事实的权威重同步（生成 / 销毁 / 拾取）
- [ ] 跨区域迁移 fence / ack / epoch（生产级加固）
- [ ] 掉落物与拾取（预算、冷却、满背包处理）

**区块管线**

- [ ] 实际发送路径的准备 / 就绪 / 交付窗口完全分离
- [ ] 按连接 / 世界 / 全服 wire 字节预算与公平性
- [ ] 完整有序 `ConnectionGate` 与出生 / 传送屏障
- [ ] 热列有界 delta journal（目前只有缺口计数）

**网络与协议**

- [ ] 多协议 profile 隔离（目前一包一 profile）
- [ ] 真机验证完整登录→区块→传送流程
- [ ] 真实慢客户端行为与 hook 超时故障策略

**持久化与恢复**

- [ ] 崩溃 / 断电恢复（WAL 落盘已做，恢复未验证）
- [ ] 跨重启外溢 journal（含重放与清理点）
- [ ] 脏 pin 字节 / 年龄预算与存储 IO 超时

**插件**

- [x] 双通道：Rust 内置插件 + C-ABI 动态插件
- [ ] 数据定义版本化显式 bundle API
- [ ] ABI 能力协商与安全卸载排空（加固路径）

**性能基线**

- [ ] 真机 RSS / 分配器 / 吞吐 / tick 延迟基线
- [ ] profile 指导的内存与 CPU 优化（NBT 共享、payload 计量、索引）

## 13. 贡献规则

1. 区域拥有修改权；锁只保护内存，不授予写权限。
2. 世界修改不用全局单线程提交；属主内有序，属主间并行。
3. `await` 前先快照并释放所有 guard；绝不跨区域/`await` 持锁。
4. 顺序必须显式；不依赖注册顺序、哈希表顺序、worker 完成顺序。
5. 守住游戏/网络、数据/行为、格式/契约边界；版本包是数据，不是代码。
6. 一切有界：条数、字节、在途、年龄、背压、取消、排空。
7. 最小改动；给所有权、过期拒绝、顺序、重试、饱和、取消、保存确认加定向测试。
