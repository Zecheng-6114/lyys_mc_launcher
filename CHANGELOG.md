# 更新日志

本文件的格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

## [未发布]

## [v0.0.2-alpha] - 2026-11-14

### 新增
- 多语言（i18n）支持：新增简体中文 / English / 日本語三套界面文案，缺失键依次回退
  en-US 与键名本身，界面语言可在全局配置中切换
- 图形界面多任务模型：重构任务体系并新增任务页面，支持多任务并行执行、
  逐行实时日志与任务取消
- 下载页重组：支持搜索 Modrinth 整合包并直接导入

### 变更
- 界面重构为暗色极简双栏布局
  （[#1](https://github.com/Zecheng-6114/lyys_mc_launcher/pull/1)）
- 下载页加载器由文本输入改为选择器，并精简表单文案
- 新增英文与日文 README，三种语言页面互相跳转
- 项目以 GPL-3.0 许可证发布，并清理源码注释
- 升级核心依赖

## [v0.0.1-alpha] - 2026-11-11

首个发布版本。

### 新增
- 双模式入口：无参数启动 GPUI 图形界面，带参数进入命令行模式
  （launch / download / search / add / import 五个子命令）
- 版本安装：一键安装原版，或叠加 Fabric / Forge / NeoForge 加载器
- 启动流程：解析 inheritsFrom 继承链并合并库与启动参数，补全版本文件与资源，
  准备 natives 后拉起游戏
- 整合包导入：支持 Modrinth（.mrpack）、CurseForge、MCBBS 与服务端整合包，
  支持 --dry-run 预览
- 模组管理：通过 Modrinth API 搜索并安装 mod，自动处理依赖（可选依赖可控）
- 实例隔离：每个实例可独立使用自己的 versions/<实例> 目录作为游戏目录
- 下载完整性：SHA1 / SHA512 校验与失败重试，支持 --mirror / --maven 镜像加速
- 离线启动：由用户名派生离线 UUID
- 构建发布工作流：推送 v* 标签时在 Windows / Linux 构建并发布

### 修复
- 适配 zip 9 API 并升级依赖

[未发布]: https://github.com/Zecheng-6114/lyys_mc_launcher/compare/v0.0.2-alpha...HEAD
[v0.0.2-alpha]: https://github.com/Zecheng-6114/lyys_mc_launcher/compare/v0.0.1-alpha...v0.0.2-alpha
[v0.0.1-alpha]: https://github.com/Zecheng-6114/lyys_mc_launcher/releases/tag/v0.0.1-alpha
