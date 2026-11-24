#!/bin/bash
cd "$(dirname "$0")" || exit 1

chmod +x ./drzk-pay-sim 2>/dev/null

./drzk-pay-sim "$@"

echo
echo "模拟器已退出。按回车关闭窗口。"
read -r
