# Installation

## Prerequisites

- **Rust 1.85+** — install via [rustup](https://rustup.rs/) (the project uses edition 2024).
- **FFmpeg 8.x** — both the libraries (rustjay-mosh links `ffmpeg-next`) and the `ffmpeg` CLI binary.
- A GPU with **Metal / Vulkan / DX12** support for the wgpu preview.
- A C/C++ build toolchain for linking FFmpeg and wgpu's native backends.

## Platform setup

### macOS

```sh
# FFmpeg libraries + CLI, and pkg-config so the build can find them
brew install ffmpeg pkg-config
xcode-select --install      # if you don't already have the CLT
```

Rendering uses **Metal** — no extra GPU drivers needed.

### Linux

```sh
# Debian / Ubuntu
sudo apt install ffmpeg libavcodec-dev libavformat-dev libavutil-dev \
                 libavfilter-dev libswscale-dev pkg-config \
                 vulkan-tools libvulkan-dev build-essential

# Arch
sudo pacman -S ffmpeg pkgconf vulkan-tools vulkan-icd-loader
```

Rendering uses **Vulkan** — make sure your GPU drivers are current.

### Windows

Install FFmpeg 8.x (shared build) and make sure its `bin` directory is on `PATH`, plus the **Visual Studio Build Tools** (MSVC). Rendering uses **Vulkan or DX12**; keep your GPU drivers up to date.

> If the build fails with a `pkg-config` / `libavutil` error, FFmpeg's dev libraries or `pkg-config` aren't visible to the build. On macOS the usual fix is `brew install pkg-config ffmpeg`.

## Build & run

```sh
git clone https://github.com/BlueJayLouche/rustjay-mosh
cd rustjay-mosh
cargo run --release
```

The first build pulls and compiles FFmpeg and wgpu and takes a few minutes. Subsequent runs are fast.

## Bundled FFmpeg

Release builds **bundle the `ffmpeg` CLI binary** next to the executable (inside the macOS `.app`, or alongside `ffmpeg.exe` on Windows). At runtime the app resolves a sibling `ffmpeg` first and only falls back to the one on your `PATH`. For local `cargo run`, the `PATH` copy is used — which is why the CLI install above matters even though the app links the libraries directly.

You're ready. Head to [Your First Mosh](getting-started/README.md).
