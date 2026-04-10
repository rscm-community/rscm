# rscm

**Reproducible System Configuration Manager**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-2024-orange.svg?logo=rust)](https://www.rust-lang.org/)
[![Arch Linux](https://img.shields.io/badge/Arch_Linux-supported-1793d1?logo=archlinux)](https://archlinux.org/)
[![Lua 5.4](https://img.shields.io/badge/Lua-5.4-2c2d72.svg?logo=lua)](https://www.lua.org/)

A declarative system configuration manager designed for **Arch Linux** and its derivatives. rscm provides reproducible, safe, and flexible system configuration management while staying native to the Arch ecosystem (AUR, ALA, rolling releases).

## Features

- **Declarative Configuration** - Define your entire system state in Lua 5.4 DSL
- **Reproducible Builds** - Content-addressed storage (SHA256) and lock files ensure identical results across machines
- **Generation Management** - Atomic system switches with full rollback support
- **Arch Native** - Deep integration with Pacman, AUR, and Arch's rolling release model
- **Sandboxed Builds** - AUR packages are built in isolated environments (bubblewrap)
- **Multi-System Support** - Manage multiple host configurations from a single file
- **Plugin System** - Extend functionality with Rust or Lua plugins
- **Dotfiles Management** - Declarative user dotfiles with source tracking
- **Boot Integration** - Automatic systemd-boot entry generation per generation

## Quick Start

### Prerequisites

- **Arch Linux** (or derivative)
- **Rust 2024 Edition** toolchain
- Root privileges for system operations

### Installation

From source:

```bash
# Build from source
git clone https://github.com/rscm-community/rscm.git
cd rscm
cargo build --release

# Install binary
sudo install -m 755 target/release/rscm /usr/bin/rscm
```

or use aur helper like `yay`:

```bash
yay -S rscm-bin
```

### Initialize

```bash
# Initialize rscm store and configuration
sudo rscm init
```

### Basic Usage

```bash
# Build and switch to a system configuration
sudo rscm switch

# List available generations
rscm generations

# Rollback to a previous generation
sudo rscm rollback

# Check system status
rscm status

# Garbage collect old generations
sudo rscm gc
```

## Configuration

The main configuration file is located at `/etc/rscm/configuration.lua`. Here's a minimal example:

```lua
system {
    hostname = "workstation",
    timezone = "Asia/Shanghai",
    locale = "en_US.UTF-8",
    locales = {
        "en_US.UTF-8",
        "zh_CN.UTF-8",
    }
}

packages {
    "vim",
    "git",
    "htop",

    neovim = {
        version = "v0.12.1",
    },
}

services {
    sshd = {
        enable = true,
    },
}

users {
    alice = {
        uid = 1000,
        groups = { "wheel", "docker" },
        shell = "/bin/zsh",
    },
}
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `rscm init` | Initialize rscm store and directories |
| `rscm build <system>` | Build a system configuration |
| `rscm switch <system>` | Build and activate a system configuration |
| `rscm generations` | List all generations |
| `rscm rollback` | Rollback to a previous generation |
| `rscm gc` | Garbage collect unreferenced store paths |
| `rscm status` | Show current system status |
| `rscm lock` | Manage lock files |
| `rscm pkg` | Package management operations |
| `rscm dotfiles` | Dotfiles management |
| `rscm plugin` | Plugin management |
| `rscm cache` | Cache management |
| `rscm check` | Validate configuration |
| `rscm outputs` | Show configuration outputs |
| `rscm toolchain` | Toolchain management |

## Filesystem Layout

```
/etc/rscm/
├── configuration.lua    # Main configuration entry
├── rscm.lock            # Auto-generated lock file
└── modules/             # Local Lua modules

/rscm/
├── store/
│   ├── content/         # Content-addressed storage (SHA256)
│   ├── packages/        # Package metadata
│   └── generations/     # System generations (symlink trees)
├── cache/               # Package cache (AUR, ALA)
├── sources/             # External source cache
├── locks/               # Lock repository
└── current-system -> /rscm/store/generations/42/
```

## Design Philosophy

- **Pragmatic First** - Reuse mature host tools (git, systemd, curl) instead of reimplementing
- **Arch Native** - Embrace Arch's rolling release and AUR ecosystem
- **Reproducible** - Lock files and content-addressed storage ensure reproducibility
- **Secure Isolation** - Sandboxed builds protect the host system
- **Progressive Adoption** - Start managing parts of your system, transition gradually
- **Declarative Management** - Only explicitly declared parts are managed by rscm

## Project Structure

```
src/
├── main.rs              # CLI entry point
├── lib.rs               # Library root
├── cli/                 # Command-line interface (clap)
├── config/              # Configuration parsing and validation
├── lua/                 # Lua 5.4 engine (mlua) and builtins
├── store/               # Content-addressed storage system
├── pkg/                 # Package managers (Pacman, AUR)
├── lock/                # Lock file generation and resolution
├── boot/                # Boot loader integration
├── service.rs           # Service management
├── user.rs              # User management
├── system_config.rs     # System configuration
├── toolchain.rs         # Toolchain management
└── cache.rs             # Cache management
```

## Technical Stack

- **Language**: Rust 2024 Edition
- **CLI**: clap (derive)
- **Configuration**: Lua 5.4 via mlua
- **Serialization**: serde, toml, serde_json
- **Networking**: reqwest
- **Crypto**: SHA256 (sha2)

## Comparison with NixOS

| Feature | rscm | NixOS |
|---------|------|-------|
| Target | Arch Linux | NixOS |
| Config Language | Lua 5.4 | Nix |
| Package Sources | Pacman + AUR | nixpkgs |
| Store Path | `/rscm/store` | `/nix/store` |
| Rolling Release | Yes (Arch native) | No |
| AUR Support | Native | Limited |

## License

[MIT](LICENSE)

## Links

- [Repository](https://github.com/rscm-community/rscm)
