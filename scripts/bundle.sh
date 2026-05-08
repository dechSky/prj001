#!/bin/bash
# macOS .app 번들 생성 스크립트.
# 사용법: ./scripts/bundle.sh [debug|release]
# 결과: target/<profile>/pj001.app
#
# 학습 목적으로 cargo-bundle 같은 도구 의존성 추가하지 않고 수동 작성.
# Info.plist 키는 Apple Developer 문서 + Tom Mewett 가이드에 따른 최소 GUI 앱 요건.

set -euo pipefail

PROFILE="${1:-debug}"
BIN_NAME="pj001"

if [ "$PROFILE" != "debug" ] && [ "$PROFILE" != "release" ]; then
    echo "usage: $0 [debug|release]" >&2
    exit 1
fi

# 스크립트 위치 → 프로젝트 루트
cd "$(dirname "$0")/.."

# 빌드
if [ "$PROFILE" = "release" ]; then
    cargo build --release
else
    cargo build
fi

APP_DIR="target/${PROFILE}/${BIN_NAME}.app"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"

# 깨끗하게 재생성
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR"

# 바이너리 복사 (심링크 아님 — open이 ProcessSerialNumber 부여하려면 실파일)
cp "target/${PROFILE}/${BIN_NAME}" "${MACOS_DIR}/${BIN_NAME}"
chmod +x "${MACOS_DIR}/${BIN_NAME}"

# Info.plist
cat > "${CONTENTS_DIR}/Info.plist" <<'PLIST_EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>pj001</string>
    <key>CFBundleIdentifier</key>
    <string>com.derek.pj001</string>
    <key>CFBundleName</key>
    <string>pj001</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleVersion</key>
    <string>0.1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST_EOF

# ad-hoc 코드 서명 (로컬 실행용. Gatekeeper 차단 회피).
codesign --force --deep --sign - "${APP_DIR}" 2>/dev/null || true

# plist 문법 검증
plutil -lint "${CONTENTS_DIR}/Info.plist"

echo "bundled: ${APP_DIR}"
echo ""
echo "실행 옵션:"
echo "  open ${APP_DIR}                              # 백그라운드 실행 (로그 미출력)"
echo "  ${MACOS_DIR}/${BIN_NAME}                     # 직접 실행 (로그 stdout)"
echo "  open -a ${APP_DIR} --stdout /tmp/pj001.log   # 백그라운드 + 로그 파일"
