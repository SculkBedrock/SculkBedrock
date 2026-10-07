<div align="center">
  <a href="https://github.com/SculkBedrock/SculkBedrock">
    <img src="repo_banner.svg" alt="SculkBedrock">
  </a>
  <h3 align="center">用 Rust 编写的下一代 Minecraft: Bedrock Edition 服务端软件</h3>

  ### [English](README.md) | 简体中文
</div>

## 🎉简介🎉

### SculkBedrock 是一个用 Rust 编写的 Minecraft: Bedrock Edition 第三方服务端软件。

它采用自研 **ECS** 架构，带有 Bedrock 协议栈与 RakNet 传输，使用版本包（`.scver`）承载游戏数据，使用 LevelDB 存档。

> [!IMPORTANT]
> 本项目处于非常早期的开发阶段。包含大量**未知 Bug**与**许多尚未实现的功能**。目前没有正式发行版，请不要在**生产环境**中使用。

📚 详细架构与功能说明：[DOCS_zh_CN.md](DOCS_zh_CN.md)

## 🎶特性🎶

- **ECS 架构**：自研 ECS 管理游戏，正在向独立区域并行 tick 演进。
- **版本包**：服务端不写死某一个 Minecraft 版本，把版本包放入 `version_packs/`，启动时自动加载方块、物品与世界生成数据。
- **数据驱动的方块/物品**：单方块 JSON 定义，启动时编译为只读快照。
- **带预算的区块管线**：共享加载、共享编码、有序交付，全部有界、有回执、有背压。
- **双通道插件**：Rust 内置插件与 C-ABI 动态库插件。

## 🎆开始使用🎆

需要 Cargo 构建。
[下载 Rust](https://www.rust-lang.org/zh-CN/learn/get-started)

### 1. 获取版本包（必需）

服务端运行**需要版本包**，否则无法正常启动和进服。

- 版本包仓库：[SculkBedrock-VersionPack](https://github.com/SculkBedrock/SculkBedrock-VersionPacks)
- 将下载的 `.scver` 文件放入本仓库的 `version_packs/` 目录。
- 版本包内的插件代码位于版本包仓库的 `crates/ur_vanilla` 下。

### 2. 运行 / 构建

```shell
cargo run -p sc_bootstrap
```

```shell
cargo check -p sc_bootstrap 
cargo build -p sc_bootstrap
```

常用配置在 `server_properties.toml`（端口、MOTD、视距、每 tick 区块数、出生阈值）。

> [!NOTE]
> 目前需要先用 Minecraft 生成 `level.dat` 并放入世界文件夹（例如 `worlds/OverWorld/level.dat`），才能启动服务端。

## ✅当前状态✅

**已完成（部分可用，非生产就绪）：**

- [x] 登录与出生
- [x] 区块订阅 / 编码 / 有序交付循环与回执
- [x] RakNet 预算、LevelDB 写回与保存确认
- [x] 数据驱动方块定义与服务端权威挖掘计时

**未完成：**

- [ ] 多区域并行 tick、准备/交付完全分离
- [ ] 崩溃恢复、库存事务、BlockEntity 落地
- [ ] 多协议支持、真机性能基线

完整内容：[DOCS_zh_CN.md](DOCS_zh_CN.md)

## 🙏特别鸣谢🙏

感谢以下开源项目在协议、存档、世界生成和服务端架构上的参考与启发（排名不分先后）：

- [Nukkit](https://github.com/CloudburstMC/Nukkit)
- [PowerNukkit](https://github.com/PowerNukkit/PowerNukkit)
- [PowerNukkitX](https://github.com/PowerNukkitX/PowerNukkitX)
- [PocketMine-MP](https://github.com/pmmp/PocketMine-MP)
- [JSPrismarine](https://github.com/JSPrismarine/JSPrismarine)

## 👉反馈👈

欢迎报告 Bug 或提出建议。请附带版本包身份、协议版本、日志与复现步骤，不要粘贴日志中的登录 token。

## 📄许可证📄

如无特殊说明，项目内容以 GPL-3.0 开源，见 `LICENSE`。

## ⚖️免责声明⚖️

**此项目与 Mojang Studios、Microsoft 和网易公司并无关联。**
