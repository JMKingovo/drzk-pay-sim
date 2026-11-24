#!/bin/bash
# 双击运行；首次使用若被系统拦截，见同目录「使用说明.txt」
cd "$(dirname "$0")" || exit 1

# 从压缩包解出来会带隔离属性，清掉才能直接运行
xattr -d com.apple.quarantine ./drzk-pay-sim 2>/dev/null
xattr -d com.apple.quarantine "./启动.command" 2>/dev/null

./drzk-pay-sim "$@"

echo
echo "模拟器已退出。按回车关闭窗口。"
read -r
