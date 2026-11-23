# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- 账号登录：支持 Microsoft 正版账号（设备码登录）与离线账号，可查看、切换和移除账号。
- 游戏安装与启动：下载 Minecraft 版本文件并启动游戏，自动检测可用的 Java。
- 实例管理：扫描已安装的版本，可选择实例启动；开启版本隔离后每个实例使用独立的游戏目录和配置。
- 模组加载器：支持安装 Fabric、Quilt、Forge 和 NeoForge，并可指定加载器版本。
- 图形界面：安装、启动、内容、设置四个页面，提供版本列表、安装进度显示和主题背景。
- 配置管理：支持全局配置与单实例覆盖，可在设置页中修改 Minecraft 目录、Java 路径和版本隔离。
- 内容下载：从 Modrinth 搜索并安装模组、资源包、光影包和数据包，自动安装所需依赖，支持更新与卸载。
- 命令行：提供账号、安装、启动、搜索、下载、配置和图形界面等命令，便于脚本化操作。
- 多语言：支持 English、简体中文、繁體中文和日本語，可跟随系统语言或手动切换。
- 发布构建：自动构建 Linux 与 Windows 版本，生成带 SHA256 校验文件的压缩包。
