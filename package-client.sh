#!/usr/bin/env bash
# 打包三平台客户端：每个平台一个目录（二进制 + 启动器 + 使用说明），再压成 zip
#
#   ./package-client.sh
#
# 产物在 dist/client/：
#   drzk-pay-sim-windows-x64.zip
#   drzk-pay-sim-macos-arm64.zip
#   drzk-pay-sim-linux-x64.zip
set -euo pipefail
cd "$(dirname "$0")"

echo "==> 构建三平台产物"
./build-all.sh

OUT=dist/client
rm -rf "$OUT"
mkdir -p "$OUT"

pack() {
  local name="$1" bin_src="$2" bin_name="$3" launcher="$4"
  local dir="$OUT/$name"
  mkdir -p "$dir"
  cp "$bin_src" "$dir/$bin_name"
  chmod +x "$dir/$bin_name" 2>/dev/null || true
  cp "client/$launcher" "$dir/$launcher"
  chmod +x "$dir/$launcher" 2>/dev/null || true
  cp "client/使用说明.txt" "$dir/使用说明.txt"

  # 用 Python 打 zip：macOS 的 zip 不会给中文文件名打 UTF-8 标记，
  # 到 Windows 资源管理器里会显示成乱码。
  python3 - "$OUT" "$name" <<'PY'
import os, sys, zipfile
out, name = sys.argv[1], sys.argv[2]
root = os.path.join(out, name)
with zipfile.ZipFile(os.path.join(out, name + ".zip"), "w", zipfile.ZIP_DEFLATED) as z:
    for dirpath, _, files in os.walk(root):
        for f in sorted(files):
            full = os.path.join(dirpath, f)
            z.write(full, os.path.relpath(full, out))
PY
  echo "    $OUT/$name.zip"
}

pack "drzk-pay-sim-windows-x64"  "dist/drzk-pay-sim-windows-x86_64.exe" "drzk-pay-sim.exe" "启动.cmd"
pack "drzk-pay-sim-macos-arm64"  "dist/drzk-pay-sim-macos-arm64"        "drzk-pay-sim"     "启动.command"
pack "drzk-pay-sim-linux-x64"    "dist/drzk-pay-sim-linux-x86_64"       "drzk-pay-sim"     "启动.sh"

echo
echo "==> 客户端包:"
ls -lh "$OUT"/*.zip | awk '{printf "    %-46s %s\n", $NF, $5}'
