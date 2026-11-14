# lyys_mc_launcher

![License](https://img.shields.io/badge/License-GPL_v3-blue) ![Rust](https://img.shields.io/badge/Rust-1.85%2B-green) ![GPUI](https://img.shields.io/badge/GPUI-0.9.1-lightblue) ![Minecraft](https://img.shields.io/badge/Minecraft-Java_Edition-green) ![Platform](https://img.shields.io/badge/Platform-Windows_%7C_Linux-lightgrey) ![Version](https://img.shields.io/badge/Version-v0.0.2--alpha-red)

[简体中文](README.md) | [English](README.en.md) | [日本語](README.ja.md)

基于 Rust 与 GPUI 的 Minecraft 启动器，提供图形界面与命令行双模式，支持原版及 Fabric / Forge / NeoForge 的下载、安装与启动，并可直接导入 Modrinth、CurseForge、MCBBS 及服务端整合包。

## 功能特性

- **双模式**：无参数启动图形界面，带参数进入命令行模式
- **版本安装**：一键安装原版，或叠加 Fabric / Forge / NeoForge 加载器
- **版本继承链**：完整解析 `inheritsFrom` 依赖链并合并库与启动参数
- **整合包导入**：支持 Modrinth（`.mrpack`）、CurseForge、MCBBS 与服务端整合包，支持 `--dry-run` 预览
- **模组管理**：通过 Modrinth API 搜索并安装 mod，自动处理依赖（可选依赖可控）
- **实例隔离**：每个实例可独立使用自己的 `versions/<实例>` 目录作为游戏目录
- **镜像加速**：`--mirror` / `--maven` 可重写 Mojang 资源与 Maven 库下载源
- **完整性校验**：下载文件带 SHA1 / SHA512 校验与重试
- **离线启动**：由用户名派生离线 UUID
- **正版登录**：微软设备码流程登录，令牌自动续期；已登录时启动自动使用正版身份，`--offline` / `--username` 可强制离线
- **多语言**：简体中文 / English / 日本語
- **任务并行**：图形界面支持多任务并行执行与实时日志

## 环境要求

- Rust 1.85 或更高版本（edition 2024）
- Windows / Linux；Linux 构建需 `cmake`、`clang`、`pkg-config` 及 Wayland / X11 相关开发库（详见 `.github/workflows/release.yml`）

## 构建

```bash
cargo build --release
```

产物位于 `target/release/lyys_mc_launcher`（Windows 为 `lyys_mc_launcher.exe`）。

## 使用

### 图形界面

直接运行可执行文件（不带参数）即可打开图形界面。

### 命令行

```text
launch:   lyys_mc_launcher launch --dir <game_dir> --name <instance> [--username <name>] [--memory <MB>] [--isolate] [--offline] [--mirror <url>] [--maven <url>] [--java <path>]
download: lyys_mc_launcher download --dir <game_dir> --version <version> [--loader <fabric|forge|neoforge>] [--loader-version <v>] [--name <instance>] [--java <path>] [--mirror <url>] [--maven <url>]
search:   lyys_mc_launcher search --query <text> --type <mod|modpack|resourcepack|shader|datapack> [--loader <a,b>] [--version <x,y>] [--limit <n>]
add:      lyys_mc_launcher add --dir <game_dir> --name <instance> --slug <slug> --type <kind> [--with-optional]
import:   lyys_mc_launcher import --dir <game_dir> (--file <archive> | --url <url> | --modrinth <slug>) [--pack-version <id>] [--name <instance>] [--java <path>] [--loader <fabric|forge|neoforge>] [--loader-version <v>] [--with-optional] [--dry-run] [--mirror <url>] [--maven <url>]
login:    lyys_mc_launcher login
logout:   lyys_mc_launcher logout
```

示例：

```bash
# 下载并安装 1.21.4 原版实例
lyys_mc_launcher download --dir D:\game\minecraft --version 1.21.4

# 在该实例上叠加 Fabric 并启动
lyys_mc_launcher download --dir D:\game\minecraft --version 1.21.4 --loader fabric --name 1.21.4-fabric
lyys_mc_launcher launch --dir D:\game\minecraft --name 1.21.4-fabric --username Steve --isolate

# 微软正版登录（浏览器打开链接并输入代码后自动完成）
lyys_mc_launcher login

# 从 Modrinth 导入整合包
lyys_mc_launcher import --dir D:\game\minecraft --modrinth <pack-slug>
```

## 配置

全局配置位于 `<基目录>/lyys_mc_launcher/config.json`：

| 字段 | 说明 |
|---|---|
| `game_dir` | 默认游戏根目录 |
| `username` | 默认离线用户名 |
| `memory` | 内存上限（MB） |
| `java` | Java 可执行文件路径 |
| `mirror` | Mojang 资源镜像 |
| `maven` | Maven 库镜像 |
| `isolate` | 是否默认开启实例隔离 |
| `language` | 界面语言（`zh-CN` / `en-US` / `ja-JP`） |

基目录的确定规则：从可执行文件所在目录逐级向上查找包含 `Cargo.toml` 的目录；开发环境即为项目根目录，否则使用当前工作目录。

每个实例还可拥有独立配置：

- `versions/<实例>/launch.json`：实例级启动参数（用户名、内存、Java、镜像、隔离）
- `versions/<实例>/lyys-instance.json`：实例元数据（隔离标记、整合包来源记录）
- `<基目录>/lyys_mc_launcher/account.json`：微软正版登录令牌（由 `login` 写入，启动时自动使用）

运行日志写入 `<基目录>/lyys_mc_launcher/log/`。

## 项目结构

```text
src/
├── main.rs        # 入口：CLI 分发或 GUI 启动
├── cli.rs         # 命令行解析
├── gui.rs         # GPUI 图形界面
├── i18n.rs        # 多语言支持
├── locales/       # 多语言资源（en-US / zh-CN / ja-JP）
├── version.rs     # 版本 JSON 解析与继承链合并
├── download.rs    # 下载、校验、镜像重写
├── loader.rs      # Fabric / Forge / NeoForge 安装
├── installer.rs   # Forge 安装器 processor 执行
├── metadata.rs    # 加载器版本工件解析
├── modrinth.rs    # Modrinth 搜索与安装
├── modpack.rs     # 整合包导入（modpack/ 为各 provider 实现）
├── instance.rs    # 实例配置与隔离
├── launch.rs      # 启动参数构建
├── microsoft.rs   # 微软正版登录（设备码流程）
└── account.rs     # 离线 UUID
```

## 许可证

本项目基于 [GPL-3.0](LICENSE) 许可证发布。

## 声明

LYYS MC Launcher 是一个非官方的 Minecraft 启动器，与 Mojang Studios、Microsoft 或他们位于中国大陆的代理公司之间并无任何从属或关联。
