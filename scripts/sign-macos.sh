#!/usr/bin/env bash
# 對 macOS 的 LocalPDF.app 做由內而外的完整簽章（含 Resources/ 底下的 gs、qpdf、pyext）。
#
# 為什麼需要這支腳本：
#   Tauri 打包時只會簽 Contents/MacOS/ 與 Frameworks/，不會深入 Contents/Resources/
#   去找我們放進去的 ghostscript/bin/gs、qpdf/bin/qpdf、qpdf/lib/libqpdf.*.dylib，
#   也不會簽 Python 擴充包裡上百個 .so/.dylib。只要漏簽任何一個二進位檔，
#   Apple 的 notarytool 公證就會退件，Gatekeeper 也會顯示「已損毀，無法打開」。
#
# 用法:
#   # 開發本機測試（ad-hoc 簽章，不需付費帳號）
#   bash scripts/sign-macos.sh
#
#   # 正式發佈（Developer ID + Hardened Runtime）
#   APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
#     bash scripts/sign-macos.sh
set -euo pipefail

[[ "$(uname -s)" == "Darwin" ]] || { echo "只能在 macOS 上執行" >&2; exit 1; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${1:-$ROOT/src-tauri/target/release/bundle/macos/LocalPDF.app}"
ENTITLEMENTS="$ROOT/src-tauri/entitlements.plist"
IDENTITY="${APPLE_SIGNING_IDENTITY:--}"

[[ -d "$APP" ]] || { echo "找不到 $APP，請先執行 npm run tauri build" >&2; exit 1; }

SIGN_FLAGS=(--force --sign "$IDENTITY")
if [[ "$IDENTITY" != "-" ]]; then
  SIGN_FLAGS+=(--options runtime --timestamp)
fi

echo "[sign] 簽章身分：$IDENTITY"
echo "[sign] 目標套件：$APP"

# 1. 先簽所有動態函式庫（.dylib / .so），由深到淺
while IFS= read -r -d '' lib; do
  codesign "${SIGN_FLAGS[@]}" "$lib"
done < <(find "$APP/Contents/Resources" \( -name "*.dylib" -o -name "*.so" \) -type f -print0 2>/dev/null)

# 2. 再簽內嵌的執行檔（gs、qpdf、python3 等）
for bin in \
  "$APP/Contents/Resources/ghostscript/bin/gs" \
  "$APP/Contents/Resources/qpdf/bin/qpdf" \
  "$APP/Contents/Resources/pyext/bin/python3" \
  "$APP/Contents/Resources/pyext/bin/python3.11"
do
  if [[ -f "$bin" && ! -L "$bin" ]]; then
    codesign "${SIGN_FLAGS[@]}" --entitlements "$ENTITLEMENTS" "$bin"
  fi
done

# 3. 最後簽整個 .app bundle（不要加 --deep，否則會用外層設定覆蓋內層）
codesign "${SIGN_FLAGS[@]}" --entitlements "$ENTITLEMENTS" "$APP"

echo "[sign] 驗證簽章…"
codesign --verify --deep --strict --verbose=2 "$APP"
echo "[sign] 完成"
