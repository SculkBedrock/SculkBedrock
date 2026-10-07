<div align="center">
  <a href="https://github.com/SculkBedrock/SculkBedrock">
    <img src="repo_banner.svg" alt="SculkBedrock">
  </a>
  <h3 align="center">The next-gen Minecraft: Bedrock Edition server software written in Rust</h3>

  ### English | [简体中文](README_zh_CN.md)
</div>

## 🎉 Introduction 🎉

### SculkBedrock is a third-party server software for Minecraft: Bedrock Edition written in Rust.

It uses **ECS** architecture, a Bedrock protocol stack with RakNet, version packs (`.scver`) for game data, and LevelDB saves.

> [!IMPORTANT]
> This project is in a very early stage of development. 
>
> It contains **unknown bugs** and **many unimplemented features**. There is no official release. Do not use it in **production**.

📚 Detailed architecture and capability guide: [DOCS.md](DOCS.md)

## 🎶 Features 🎶

- **ECS architecture**: ECS for game management, evolving toward parallel ticking of independent areas.
- **Version packs**: the server does not hardcode one Minecraft version. Drop a version pack into `version_packs/` and it loads blocks, items, and worldgen data at startup.
- **Data-driven blocks/items**: per-block JSON definitions compiled into read-only snapshots at startup.
- **Bounded chunk pipeline**: shared loading, shared encoding, and ordered delivery with budgets and backpressure.
- **Dual plugin channels**: in-process Rust plugins and C-ABI dynamic-library plugins.

## 🎆 Getting Started 🎆

You need Cargo to build it.
[Download Rust](https://www.rust-lang.org/learn/get-started)

### 1. Get the version pack (required)

The server **requires a version pack** to start and accept players.

- Version packs: [SculkBedrock-VersionPack](https://github.com/SculkBedrock/SculkBedrock-VersionPacks)
- Put the downloaded `.scver` file into this repository's `version_packs/` directory.
- Plugin sources inside the version-pack repository live under `crates/ur_vanilla`.

### 2. Run / Build

```shell
cargo run -p sc_bootstrap
```

```shell
cargo check -p sc_bootstrap
cargo build -p sc_bootstrap
```

Settings live in `server_properties.toml` (ports, MOTD, view distance, per-tick chunk budget, spawn threshold).

> [!NOTE]
> You currently need to generate a `level.dat` with Minecraft and place it into the world folder (e.g. `worlds/OverWorld/level.dat`) before starting the server.

## ✅ Current status ✅

**Completed (partial, not production-ready):**

- [x] Login and first spawn
- [x] Chunk subscribe / encode / ordered-delivery loop with receipts
- [x] RakNet budgets, LevelDB writeback with save confirmations
- [x] Data-driven block definitions with server-authoritative break timing

**Uncompleted:**

- [ ] Multi-region parallel ticking, full prepare/delivery separation
- [ ] Crash recovery, inventory transactions, BlockEntity landing
- [ ] Multi-protocol support, live performance baselines

Full list: [DOCS.md](DOCS.md)

## 🙏 Special thanks 🙏

Thanks to the following open-source projects for reference and inspiration (in no particular order):

- [Nukkit](https://github.com/CloudburstMC/Nukkit)
- [PowerNukkit](https://github.com/PowerNukkit/PowerNukkit)
- [PowerNukkitX](https://github.com/PowerNukkitX/PowerNukkitX)
- [PocketMine-MP](https://github.com/pmmp/PocketMine-MP)
- [JSPrismarine](https://github.com/JSPrismarine/JSPrismarine)
- [NetrexMC](https://github.com/NetrexMC)
- [Bevy](https://github.com/bevyengine/bevy)
## 👉 Feedback 👈

Bug reports and suggestions are welcome. Please include the version-pack identity, protocol version, logs, and reproduction steps. Do not paste login tokens from logs.

## 📄 License 📄

If not otherwise specified, project content is open source under the GPL-3.0 license. See `LICENSE`.

## ⚖️ Disclaimer ⚖️

**This project is not affiliated with Mojang Studios, Microsoft, or NetEase.**
