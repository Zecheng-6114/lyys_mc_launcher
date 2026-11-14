# lyys_mc_launcher

![License](https://img.shields.io/badge/License-GPL_v3-blue) ![Rust](https://img.shields.io/badge/Rust-1.85%2B-green) ![GPUI](https://img.shields.io/badge/GPUI-0.9.1-lightblue) ![Minecraft](https://img.shields.io/badge/Minecraft-Java_Edition-green) ![Platform](https://img.shields.io/badge/Platform-Windows_%7C_Linux-lightgrey) ![Version](https://img.shields.io/badge/Version-v0.0.2--alpha-red)

[简体中文](README.md) | [English](README.en.md) | [日本語](README.ja.md)

Rust と GPUI で構築された Minecraft ランチャー。グラフィカルインターフェースとコマンドラインインターフェースの両方を提供し、バニラおよび Fabric / Forge / NeoForge のダウンロード・インストール・起動に対応し、Modrinth、CurseForge、MCBBS、サーバー統合パックを直接インポートできます。

## 機能

- **デュアルモード**：引数なしで起動するとグラフィカルインターフェースが開き、引数付きでコマンドラインモードになります
- **バージョンインストール**：バニラをワンクリックでインストール、または Fabric / Forge / NeoForge ローダーを追加
- **バージョン継承チェーン**：`inheritsFrom` チェーンを完全に解決し、ライブラリと起動引数をマージします
- **統合パックのインポート**：Modrinth（`.mrpack`）、CurseForge、MCBBS、サーバー統合パックに対応。`--dry-run` によるプレビューも可能
- **Mod 管理**：Modrinth API を通じて Mod を検索・インストールし、依存関係を自動処理（任意の依存関係は選択可能）
- **インスタンス分離**：各インスタンスは独自の `versions/<インスタンス>` ディレクトリをゲームディレクトリとして使用できます
- **ミラー加速**：`--mirror` / `--maven` で Mojang リソースおよび Maven リポジトリのダウンロードソースを書き換え
- **整合性チェック**：ダウンロードファイルは SHA1 / SHA512 検証とリトライ付き
- **オフライン起動**：ユーザー名からオフライン UUID を生成
- **正規ログイン**：Microsoft のデバイスコードフローでログイン。トークンは自動更新され、ログイン済みなら起動時に正規アカウントを自動使用
- **多言語対応**：簡体字中国語 / 英語 / 日本語
- **並行タスク**：GUI は複数タスクの並行実行とリアルタイムログに対応

## 要件

- Rust 1.85 以降（edition 2024）
- Windows / Linux。Linux のビルドには `cmake`、`clang`、`pkg-config`、および Wayland / X11 関連の開発ライブラリが必要です（詳細は `.github/workflows/release.yml` を参照）

## ビルド

```bash
cargo build --release
```

成果物は `target/release/lyys_mc_launcher`（Windows では `lyys_mc_launcher.exe`）にあります。

## 使用方法

### グラフィカルインターフェース

実行ファイルを引数なしで実行すると、グラフィカルインターフェースが開きます。

### コマンドライン

```text
launch:   lyys_mc_launcher launch --dir <game_dir> --name <instance> [--username <name>] [--memory <MB>] [--isolate] [--offline] [--mirror <url>] [--maven <url>] [--java <path>]
download: lyys_mc_launcher download --dir <game_dir> --version <version> [--loader <fabric|forge|neoforge>] [--loader-version <v>] [--name <instance>] [--java <path>] [--mirror <url>] [--maven <url>]
search:   lyys_mc_launcher search --query <text> --type <mod|modpack|resourcepack|shader|datapack> [--loader <a,b>] [--version <x,y>] [--limit <n>]
add:      lyys_mc_launcher add --dir <game_dir> --name <instance> --slug <slug> --type <kind> [--with-optional]
import:   lyys_mc_launcher import --dir <game_dir> (--file <archive> | --url <url> | --modrinth <slug>) [--pack-version <id>] [--name <instance>] [--java <path>] [--loader <fabric|forge|neoforge>] [--loader-version <v>] [--with-optional] [--dry-run] [--mirror <url>] [--maven <url>]
login:    lyys_mc_launcher login
logout:   lyys_mc_launcher logout
```

例：

```bash
# 1.21.4 バニラインスタンスをダウンロードしてインストール
lyys_mc_launcher download --dir D:\game\minecraft --version 1.21.4

# そのインスタンスに Fabric を追加して起動
lyys_mc_launcher download --dir D:\game\minecraft --version 1.21.4 --loader fabric --name 1.21.4-fabric
lyys_mc_launcher launch --dir D:\game\minecraft --name 1.21.4-fabric --username Steve --isolate

# Microsoft アカウントでログイン（ブラウザでリンクを開きコードを入力）
lyys_mc_launcher login

# Modrinth から統合パックをインポート
lyys_mc_launcher import --dir D:\game\minecraft --modrinth <pack-slug>
```

## 設定

グローバル設定は `<基準ディレクトリ>/lyys_mc_launcher/config.json` にあります：

| フィールド | 説明 |
|---|---|
| `game_dir` | デフォルトのゲームルートディレクトリ |
| `username` | デフォルトのオフラインユーザー名 |
| `memory` | メモリ上限（MB） |
| `java` | Java 実行ファイルのパス |
| `mirror` | Mojang リソースのミラー |
| `maven` | Maven リポジトリのミラー |
| `isolate` | デフォルトでインスタンス分離を有効にするかどうか |
| `language` | インターフェース言語（`zh-CN` / `en-US` / `ja-JP`） |

基準ディレクトリの決定ルール：実行ファイルがあるディレクトリから親ディレクトリへ辿り、`Cargo.toml` を含むディレクトリを探します。開発環境ではプロジェクトルートがそれに該当し、見つからない場合は現在の作業ディレクトリを使用します。

各インスタンスは個別の設定も持てます：

- `versions/<インスタンス>/launch.json`：インスタンスごとの起動設定（ユーザー名、メモリ、Java、ミラー、分離）
- `versions/<インスタンス>/lyys-instance.json`：インスタンスのメタデータ（分離フラグ、統合パックの記録）
- `<基準ディレクトリ>/lyys_mc_launcher/account.json`：Microsoft ログイントークン（`login` で書き込み、起動時に自動使用）

実行ログは `<基準ディレクトリ>/lyys_mc_launcher/log/` に書き込まれます。

## プロジェクト構成

```text
src/
├── main.rs        # エントリポイント：CLI ディスパッチまたは GUI 起動
├── cli.rs         # コマンドライン解析
├── gui.rs         # GPUI グラフィカルインターフェース
├── i18n.rs        # 多言語サポート
├── locales/       # 多言語リソース（en-US / zh-CN / ja-JP）
├── version.rs     # バージョン JSON の解析と継承チェーンのマージ
├── download.rs    # ダウンロード、検証、ミラー書き換え
├── loader.rs      # Fabric / Forge / NeoForge のインストール
├── installer.rs   # Forge インストーラーのプロセッサ実行
├── metadata.rs    # ローダーバージョン成果物の解決
├── modrinth.rs    # Modrinth の検索とインストール
├── modpack.rs     # 統合パックのインポート（modpack/ は各プロバイダーの実装）
├── instance.rs    # インスタンス設定と分離
├── launch.rs      # 起動引数の構築
├── microsoft.rs   # Microsoft 正規ログイン（デバイスコードフロー）
└── account.rs     # オフライン UUID
```

## ライセンス

本プロジェクトは [GPL-3.0](LICENSE) ライセンスの下で公開されています。

## 免責事項

LYYS MC Launcher は非公式の Minecraft ランチャーです。Mojang Studios、Microsoft、およびそれらの中国大陸における代理会社とは一切提携・関連しておりません。
