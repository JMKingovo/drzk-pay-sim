#!/usr/bin/env bash
# 一次性构建三个平台的产物，并归集到 dist/
#
# 首次使用需先装好交叉编译工具（macOS 上）：
#   rustup target add x86_64-unknown-linux-musl x86_64-pc-windows-gnu
#   brew install messense/macos-cross-toolchains/x86_64-unknown-linux-musl
#   brew install mingw-w64
set -euo pipefail
cd "$(dirname "$0")"

echo "==> macOS (arm64)"
cargo build --release

echo "==> Linux (x86_64, musl 静态链接，不依赖 glibc)"
cargo build --release --target x86_64-unknown-linux-musl

echo "==> Windows (x86_64, 静态 CRT，无需 MSVC 运行库)"
cargo build --release --target x86_64-pc-windows-gnu

mkdir -p dist
cp target/release/drzk-pay-sim                              dist/drzk-pay-sim-macos-arm64
cp target/x86_64-unknown-linux-musl/release/drzk-pay-sim    dist/drzk-pay-sim-linux-x86_64
cp target/x86_64-pc-windows-gnu/release/drzk-pay-sim.exe    dist/drzk-pay-sim-windows-x86_64.exe

echo
echo "==> dist/ 产物:"
ls -lh dist | tail -n +2 | awk '{printf "    %-40s %s\n", $NF, $5}'
