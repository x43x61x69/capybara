#!/usr/bin/env bash
# Build the actual emulator archive before linking any hardware firmware.
set -euo pipefail
PORT_ROOT=$(cd -- "$(dirname -- "$0")" && pwd)
export CARGO_TARGET_DIR="$PORT_ROOT/core/target"
core_features=()
if [[ -n "${H717_RECOMP_PRAM:-}" ]]; then
    core_features=(--features recomp)
fi
if [[ -z "${H717_RUSTC:-}" && -x "$PORT_ROOT/local/xtensa-rust/bin/rustc" ]]; then
    export H717_RUSTC="$PORT_ROOT/local/xtensa-rust/bin/rustc"
    export H717_CARGO="${H717_CARGO:-$PORT_ROOT/local/xtensa-rust/bin/cargo}"
    export CARGO_HOME="${CARGO_HOME:-$PORT_ROOT/local/cargo-home}"
fi
if [[ -n "${H717_RUSTC:-}" ]]; then
    export RUSTC="$H717_RUSTC"
    export RUSTC_BOOTSTRAP=1
    "${H717_CARGO:-cargo}" build --locked --release --lib "${core_features[@]}" \
        -Z build-std=core,alloc --target xtensa-esp32s3-none-elf \
        --manifest-path "$PORT_ROOT/core/Cargo.toml"
elif command -v rustup >/dev/null && rustup run esp rustc --version >/dev/null 2>&1; then
    cargo +esp build --locked --release --lib "${core_features[@]}" -Z build-std=core,alloc \
        --target xtensa-esp32s3-none-elf --manifest-path "$PORT_ROOT/core/Cargo.toml"
else
    printf '%s\n' \
        'The esp-rs Xtensa Rust toolchain is required; ordinary Homebrew Rust has no Xtensa LLVM backend.' \
        'Install rustup, espup and the esp toolchain using the official Espressif Rust instructions.' \
        'Source export-esp.sh, then run this script again.' \
        'Alternatively set H717_RUSTC to a manually installed esp-rs rustc (with matching rust-src).' >&2
    exit 2
fi
if [[ "${H717_CORE_ONLY:-0}" != 1 ]]; then
    cd "$PORT_ROOT/firmware"
    "${PIO:-pio}" run -e h717
fi
