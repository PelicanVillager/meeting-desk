#!/usr/bin/env bash
# 编译本地 OCR 桥接（Swift + 系统 Vision 框架）。
# 产物：src-tauri/binaries/meetingdesk-ocr
# 说明：只用 Xcode Command Line Tools，不需要 Homebrew、不需要下载模型。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_file="${repo_root}/src-tauri/ocr-bridge/main.swift"
out_dir="${repo_root}/src-tauri/binaries"
out_file="${out_dir}/meetingdesk-ocr"

if ! command -v swiftc >/dev/null 2>&1; then
  echo "未找到 swiftc，请先安装 Xcode Command Line Tools：xcode-select --install" >&2
  exit 1
fi

mkdir -p "${out_dir}"

# 把编译器模块缓存放在临时目录，避免写入用户缓存目录失败（也会污染 ~/.cache）
export CLANG_MODULE_CACHE_PATH="${TMPDIR:-/tmp}/meetingdesk-swift-module-cache"

# 源码未变且产物存在时跳过编译
if [[ -x "${out_file}" && "${out_file}" -nt "${source_file}" ]]; then
  echo "OCR 桥接已是最新：${out_file}"
  exit 0
fi

echo "编译 OCR 桥接 …"
swiftc -O \
  -framework Foundation \
  -framework CoreGraphics \
  -framework CoreText \
  -framework ImageIO \
  -framework Vision \
  -o "${out_file}" \
  "${source_file}"

echo "完成：${out_file}"
