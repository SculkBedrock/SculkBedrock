<div align="center">
  <a href="https://github.com/SculkBedrock/SculkBedrock">
    <img src="repo_banner.svg" alt="SculkBedrock">
  </a>
  <h3 align="center">用 Rust 编写的下一代 Minecraft: Bedrock Edition 服务端软件</h3>

  ### [English](README.md) | 简体中文
</div>

## 🎉简介🎉

### SculkBedrock 是一个用 Rust 编写的 Minecraft: Bedrock Edition 第三方服务端软件。

### ECS架构, 数据驱动, 热加载...众多特性等你发现

> [!IMPORTANT]
> 本项目处于非常早期的开发阶段。
>
> 包含大量**未知 Bug**与**许多尚未实现的功能**. 目前没有正式发行版, 请不要在**生产环境**中使用.
> 
> 由于**项目开发时间跨度较大, 作者自身水平问题及学业繁杂**, 本项目可能有 **部分架构设计/逻辑存在缺陷**, 欢迎提交pr或issue进行修正!

> [!WARNING]
> ### 单独声明:
> 当前 SculkBedrock 的 **主世界生成器** 采用了 [PowerNukkitX](https://github.com/PowerNukkitX/PowerNukkitX) 的世界生成代码.
> 
> 这只是用于验证项目功能的临时举措, 并不代表我们将会长期使用 PowerNukkitX 的代码.
>
> 在后续更新中, 我们会逐步替换掉, 并采用**基于数据驱动的Density Function JSON**.

📚 详细架构与功能说明：[架构设计文档](DOCS_zh_CN.md)

## 🎶特性🎶

- **ECS 架构**：相比于 **传统服务端**, **ECS 架构** 更加贴合现代 MCBE 服务端的设计风格(尤其是 Mojang 开始大量使用 ECS 进行游戏开发). 本项目在此基础上增加 **异步/多线程** 支持, 充分发挥CPU性能. (SC ECS 的代码设计部分参考自Bevy ECS及hecs)
- **版本包**：无需重复下载多个版本的服务端, 通过版本包即可一键切换服务端游戏版本. 数据随意修改, 轻松自定义属于您的服务器.
- **数据驱动的方块/物品**：兼容 MCBE 官方的 **数据驱动** 格式, 另有单独设计的 **Block JSON** 格式等...
- **原版特性支持**: SculkBedrock 尽可能地支持原版特性, 实现近似原版的游戏体验.
- **多线程区块管线**：共享加载、共享编码、有序交付. (目前暂不完善, 设计有待验证)
- **多线程/异步支持**: SculkBedrock 原生支持 **多线程/异步**. 网络/游戏逻辑 等通过调度器并发运行, 保障游戏流畅度.
- **基于意图的网络系统**: 通过意图解耦游戏逻辑层及网络层, 防止互相影响.
- **兼容 MCBE 官方的 Addons**: 通过 **Molang 解释器及 JavaScript 解释器**, 轻松运行 MCBE 官方的 Addons. 未来可能会进一步兼容 **我的世界: 中国版** Python插件.
- **插件系统**：**Rust 内置插件** 与 **C-ABI 动态库插件** 共存, 未来可能还会支持 **PowerNukkitX插件、TypeScript插件** 等.
- **精美的TUI界面**: 不知道有什么用, 反正看着很牛逼就对了. 
- **更多等你发现**

## 🎆开始使用🎆

需要 Cargo 构建。
[下载 Rust](https://www.rust-lang.org/zh-CN/learn/get-started)

### 1. 获取版本包（必需）

服务端运行**需要版本包**，否则无法正常启动和进服。

- 版本包仓库：[SculkBedrock-VersionPack](https://github.com/SculkBedrock/SculkBedrock-VersionPacks)
- 将下载的 `.scver` 文件放入 `version_packs/` 目录。
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

## ✅当前项目进度✅

### 网络:

- [x] 基础协议包实现
- [ ] 完整协议包实现
- [x] 登录与出生
- [x] 区块订阅 / 编码 / 有序交付循环与回执 **(部分完成)**

### 游戏逻辑:

- [x] 物品配方合成 **(部分完成)**
- [ ] ⏸️ **(正在进行)** 工作台/熔炉/切石机 等方块功能
- [ ] ⏸️ **(正在进行)** 实体摔落伤害计算/饱食度
- [ ] 物品附魔/数值

### 方块:
- [ ] Block Entity支持
- [ ] 功能方块

### 其他功能:
- [ ] ⏸️ **(正在进行)** 插件系统
- [ ] Molang解释器

#### 还有更多内容, 请参考文档: [DOCS_zh_CN.md](DOCS_zh_CN.md)

## 🙏特别鸣谢🙏

感谢以下开源项目在协议、存档、世界生成和服务端架构上的参考与启发（排名不分先后）:

- [Nukkit](https://github.com/CloudburstMC/Nukkit)
- [PowerNukkit](https://github.com/PowerNukkit/PowerNukkit)
- [PowerNukkitX](https://github.com/PowerNukkitX/PowerNukkitX)
- [PocketMine-MP](https://github.com/pmmp/PocketMine-MP)
- [JSPrismarine](https://github.com/JSPrismarine/JSPrismarine)
- [NetrexMC](https://github.com/NetrexMC)
- [Bevy](https://github.com/bevyengine/bevy)
## 👉反馈👈

欢迎报告 Bug 或提出建议. 请附带版本包身份、协议版本、日志与复现步骤, 不要粘贴日志中的登录 token.

## 📄许可证📄

如无特殊说明, 项目内容以 GPL-3.0 开源, 见 `LICENSE`. 

## ⚖️免责声明⚖️

**此项目与 Mojang AB、Microsoft 和网易公司并无关联. 亦未对其游戏/软件进行任何反编译/破解等违法行为. 所有代码基于公开文档/开源项目实现.**
