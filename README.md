<div align="center">
  <a href="https://github.com/SculkBedrock/SculkBedrock">
    <img src="repo_banner.svg" alt="SculkBedrock">
  </a>
  <h3 align="center">The next-gen Minecraft: Bedrock Edition server software written in Rust</h3>

  ### English | [简体中文](README_zh_CN.md)
</div>

## 🎉 Introduction 🎉

### SculkBedrock is a third-party server software for Minecraft: Bedrock Edition written in Rust.

### ECS architecture, data-driven, hot-loading... and many more features to discover

> [!IMPORTANT]
> This project is in a very early stage of development.
>
> It contains **unknown bugs** and **many unimplemented features**. There is no official release. Do not use it in **production**.
>
> Due to the **long development timeline of the project, the author's own skill level, and school commitments**, this project may have **some architectural design / logic flaws**. Welcome to submit PRs or issues to fix them!

> [!WARNING]
> ### Special statement:
> The current SculkBedrock **overworld generator** uses worldgen code from [PowerNukkitX](https://github.com/PowerNukkitX/PowerNukkitX).
>
> This is only a temporary measure to verify project functionality, and does not mean we will use PowerNukkitX's code long-term.
>
> In future updates, we will gradually replace it and adopt **data-driven Density Function JSON**.

📚 Detailed architecture and capability guide: [developer documentation](DOCS.md)

## 🎶 Features 🎶

- **ECS architecture**: Compared with **traditional servers**, the **ECS architecture** fits the design style of modern MCBE servers much better (especially as Mojang increasingly uses ECS for game development). On this basis, this project adds **async / multi-threading** support to fully exploit CPU performance. (SC ECS code design partially references Bevy ECS and hecs)
- **Version packs**: No need to download multiple server versions repeatedly; switch the server's game version with one click via version packs. Modify data freely and easily customize your own server.
- **Data-driven blocks/items**: Compatible with MCBE's official **data-driven** formats, plus a separately designed **Block JSON** format, etc.
- **Vanilla feature support**: SculkBedrock supports vanilla features as much as possible, aiming for a near-vanilla gameplay experience.
- **Multi-threaded chunk pipeline**: shared loading, shared encoding, ordered delivery. (Currently not fully complete; design needs verification)
- **Multi-threading / async support**: natively supported. Network / game logic run concurrently through the scheduler, ensuring smooth gameplay.
- **Intent-based network system**: decouples the game logic layer from the network layer via intents, preventing mutual interference.
- **MCBE addon compatibility**: run official MCBE addons easily via **Molang** and **JavaScript** interpreters. May further support Minecraft: China Edition Python plugins in the future.
- **Plugin system**: **Rust built-in plugins** and **C-ABI dynamic library plugins** coexist; may support PowerNukkitX plugins, TypeScript plugins, etc. in the future.
- **Beautiful TUI interface**: not sure what it's for, but it looks awesome.
- **More for you to discover**

## 🎆 Getting Started 🎆

You need Cargo to build it.
[Download Rust](https://www.rust-lang.org/learn/get-started)

### 1. Get the version pack (required)

The server **requires a version pack** to start and accept players.

- Version packs: [SculkBedrock-VersionPack](https://github.com/SculkBedrock/SculkBedrock-VersionPacks)
- Put the downloaded `.scver` file into the `version_packs/` directory.
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

## ✅ Current Progress ✅

### Network:

- [x] Basic protocol packet implementation
- [ ] Complete protocol packet implementation
- [x] Login and spawn
- [x] Chunk subscribe / encode / ordered-delivery loop with receipts **(partially complete)**

### Game logic:

- [x] Item recipe crafting **(partially complete)**
- [ ] ⏸️ **(in progress)** Crafting table / furnace / stonecutter block functions
- [ ] ⏸️ **(in progress)** Entity fall damage calculation / hunger
- [ ] Item enchantments / values

### Blocks:
- [ ] Block Entity support
- [ ] Functional blocks

### Other features:
- [ ] ⏸️ **(in progress)** Plugin system
- [ ] Molang interpreter

#### For more content, please refer to the docs: [DOCS.md](DOCS.md)

## 🙏 Special thanks 🙏

Thanks to the following open-source projects for reference and inspiration on protocols, storage, worldgen, and server architecture (in no particular order):

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

If not otherwise specified, project content is open source under the GPL-3.0 license, see `LICENSE`.

## ⚖️ Disclaimer ⚖️

**This project is not affiliated with Mojang AB, Microsoft, or NetEase. No decompilation/cracking of their games/software was performed. All code is based on public documentation / open-source projects.**