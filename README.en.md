# lyys_mc_launcher

![License](https://img.shields.io/badge/License-GPL_v3-blue) ![Rust](https://img.shields.io/badge/Rust-1.85%2B-green) ![GPUI](https://img.shields.io/badge/GPUI-0.9.1-lightblue) ![Minecraft](https://img.shields.io/badge/Minecraft-Java_Edition-green) ![Platform](https://img.shields.io/badge/Platform-Windows_%7C_Linux-lightgrey) ![Version](https://img.shields.io/badge/Version-v0.0.2--alpha-red)

[简体中文](README.md) | [English](README.en.md) | [日本語](README.ja.md)

A Minecraft launcher built with Rust and GPUI, offering both a graphical interface and a command-line interface. It supports downloading, installing, and launching Vanilla as well as Fabric / Forge / NeoForge, and can directly import Modrinth, CurseForge, MCBBS, and server modpacks.

## Features

- **Dual mode**: runs the graphical interface without arguments; enters command-line mode with arguments
- **Version installation**: one-click installation of Vanilla, or with the Fabric / Forge / NeoForge loader on top
- **Version inheritance chain**: fully resolves the `inheritsFrom` chain and merges libraries and launch arguments
- **Modpack import**: supports Modrinth (`.mrpack`), CurseForge, MCBBS, and server modpacks, with `--dry-run` preview
- **Mod management**: search and install mods via the Modrinth API, with automatic dependency handling (optional dependencies under your control)
- **Instance isolation**: each instance can use its own `versions/<instance>` directory as the game directory
- **Mirror acceleration**: `--mirror` / `--maven` rewrite Mojang resource and Maven repository download sources
- **Integrity checks**: downloads are verified with SHA1 / SHA512 and retried on failure
- **Offline play**: derives an offline UUID from the username
- **Microsoft sign-in**: official login via the device code flow with automatic token refresh; when signed in, launching uses the official identity automatically
- **Multilingual**: Simplified Chinese / English / Japanese
- **Parallel tasks**: the graphical interface supports running multiple tasks in parallel with live logs

## Requirements

- Rust 1.85 or later (edition 2024)
- Windows / Linux; on Linux, building requires `cmake`, `clang`, `pkg-config`, and Wayland / X11 development libraries (see `.github/workflows/release.yml`)

## Build

```bash
cargo build --release
```

The artifact is located at `target/release/lyys_mc_launcher` (`lyys_mc_launcher.exe` on Windows).

## Usage

### Graphical interface

Run the executable without arguments to open the graphical interface.

### Command line

```text
launch:   lyys_mc_launcher launch --dir <game_dir> --name <instance> [--username <name>] [--memory <MB>] [--isolate] [--offline] [--mirror <url>] [--maven <url>] [--java <path>]
download: lyys_mc_launcher download --dir <game_dir> --version <version> [--loader <fabric|forge|neoforge>] [--loader-version <v>] [--name <instance>] [--java <path>] [--mirror <url>] [--maven <url>]
search:   lyys_mc_launcher search --query <text> --type <mod|modpack|resourcepack|shader|datapack> [--loader <a,b>] [--version <x,y>] [--limit <n>]
add:      lyys_mc_launcher add --dir <game_dir> --name <instance> --slug <slug> --type <kind> [--with-optional]
import:   lyys_mc_launcher import --dir <game_dir> (--file <archive> | --url <url> | --modrinth <slug>) [--pack-version <id>] [--name <instance>] [--java <path>] [--loader <fabric|forge|neoforge>] [--loader-version <v>] [--with-optional] [--dry-run] [--mirror <url>] [--maven <url>]
login:    lyys_mc_launcher login
logout:   lyys_mc_launcher logout
```

Examples:

```bash
# Download and install a Vanilla 1.21.4 instance
lyys_mc_launcher download --dir D:\game\minecraft --version 1.21.4

# Add Fabric on top of that instance and launch
lyys_mc_launcher download --dir D:\game\minecraft --version 1.21.4 --loader fabric --name 1.21.4-fabric
lyys_mc_launcher launch --dir D:\game\minecraft --name 1.21.4-fabric --username Steve --isolate

# Sign in with a Microsoft account (open the link in a browser and enter the code)
lyys_mc_launcher login

# Import a modpack from Modrinth
lyys_mc_launcher import --dir D:\game\minecraft --modrinth <pack-slug>
```

## Configuration

The global configuration is located at `<base dir>/lyys_mc_launcher/config.json`:

| Field | Description |
|---|---|
| `game_dir` | Default game root directory |
| `username` | Default offline username |
| `memory` | Memory limit (MB) |
| `java` | Path to the Java executable |
| `mirror` | Mirror for Mojang resources |
| `maven` | Mirror for Maven repositories |
| `isolate` | Whether instance isolation is enabled by default |
| `language` | Interface language (`zh-CN` / `en-US` / `ja-JP`) |

The base directory is resolved as follows: starting from the directory containing the executable, walk up until a directory that contains `Cargo.toml` is found; in a development environment this is the project root, otherwise the current working directory is used.

Each instance can also have its own configuration:

- `versions/<instance>/launch.json`: per-instance launch settings (username, memory, Java, mirrors, isolation)
- `versions/<instance>/lyys-instance.json`: instance metadata (isolation flag, modpack provenance)
- `<base dir>/lyys_mc_launcher/account.json`: Microsoft sign-in tokens (written by `login`, used automatically at launch)

Run logs are written to `<base dir>/lyys_mc_launcher/log/`.

## Project Structure

```text
src/
├── main.rs        # Entry point: CLI dispatch or GUI startup
├── cli.rs         # Command-line parsing
├── gui.rs         # GPUI graphical interface
├── i18n.rs        # Multilingual support
├── locales/       # Locale resources (en-US / zh-CN / ja-JP)
├── version.rs     # Version JSON parsing and inheritance-chain merging
├── download.rs    # Downloads, verification, mirror rewriting
├── loader.rs      # Fabric / Forge / NeoForge installation
├── installer.rs   # Forge installer processor execution
├── metadata.rs    # Loader artifact resolution
├── modrinth.rs    # Modrinth search and installation
├── modpack.rs     # Modpack import (modpack/ implements each provider)
├── instance.rs    # Instance configuration and isolation
├── launch.rs      # Launch argument construction
├── microsoft.rs   # Microsoft sign-in (device code flow)
└── account.rs     # Offline UUID
```

## License

This project is licensed under [GPL-3.0](LICENSE).

## Disclaimer

LYYS MC Launcher is an unofficial Minecraft launcher and is not affiliated with or associated with Mojang Studios, Microsoft, or their agent companies in mainland China.
