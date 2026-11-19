#!/usr/bin/env bash
# ==============================================================================
# Build Standalone Android APK for xChat
# Produces signed dist/xchat-v${VERSION}.apk and dist/xchat.apk
# ==============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
ANDROID_DIR="${ROOT_DIR}/android"
BUILD_DIR="${ANDROID_DIR}/build"
DIST_DIR="${ROOT_DIR}/dist"

# 0. Locate Android SDK
ANDROID_SDK="${ANDROID_HOME:-}"
if [ -z "$ANDROID_SDK" ] || [ ! -d "$ANDROID_SDK" ]; then
    if [ -n "${ANDROID_SDK_ROOT:-}" ] && [ -d "${ANDROID_SDK_ROOT:-}" ]; then
        ANDROID_SDK="$ANDROID_SDK_ROOT"
    elif [ -d "/usr/local/lib/android/sdk" ]; then
        ANDROID_SDK="/usr/local/lib/android/sdk"
    elif [ -d "${HOME}/Library/Android/sdk" ]; then
        ANDROID_SDK="${HOME}/Library/Android/sdk"
    elif [ -d "${HOME}/Android/Sdk" ]; then
        ANDROID_SDK="${HOME}/Android/Sdk"
    elif [ -d "/opt/android-sdk" ]; then
        ANDROID_SDK="/opt/android-sdk"
    fi
fi

if [ -z "$ANDROID_SDK" ] || [ ! -d "$ANDROID_SDK" ]; then
    echo "[!] Error: Android SDK not found. Set ANDROID_HOME or ANDROID_SDK_ROOT."
    exit 1
fi

# Locate latest build-tools
BUILD_TOOLS_VERSION=""
if [ -d "${ANDROID_SDK}/build-tools" ]; then
    BUILD_TOOLS_VERSION="$(ls -1 "${ANDROID_SDK}/build-tools" 2>/dev/null | sort -V | tail -n 1)"
fi
BUILD_TOOLS="${ANDROID_SDK}/build-tools/${BUILD_TOOLS_VERSION}"

# Locate Android platform JAR (prefer android-34, fallback to highest available)
ANDROID_JAR="${ANDROID_SDK}/platforms/android-34/android.jar"
if [ ! -f "$ANDROID_JAR" ] && [ -d "${ANDROID_SDK}/platforms" ]; then
    LATEST_PLATFORM="$(ls -1 "${ANDROID_SDK}/platforms" 2>/dev/null | sort -V | tail -n 1)"
    if [ -n "$LATEST_PLATFORM" ]; then
        ANDROID_JAR="${ANDROID_SDK}/platforms/${LATEST_PLATFORM}/android.jar"
    fi
fi

if [ ! -f "$ANDROID_JAR" ]; then
    echo "[!] Error: Android platform jar not found at ${ANDROID_JAR}"
    exit 1
fi

AAPT2="${BUILD_TOOLS}/aapt2"
D8="${BUILD_TOOLS}/d8"
ZIPALIGN="${BUILD_TOOLS}/zipalign"
APKSIGNER="${BUILD_TOOLS}/apksigner"

[ ! -x "$AAPT2" ] && AAPT2="$(command -v aapt2 2>/dev/null || true)"
[ ! -x "$D8" ] && D8="$(command -v d8 2>/dev/null || true)"
[ ! -x "$ZIPALIGN" ] && ZIPALIGN="$(command -v zipalign 2>/dev/null || true)"
[ ! -x "$APKSIGNER" ] && APKSIGNER="$(command -v apksigner 2>/dev/null || true)"

if [ -z "$AAPT2" ] || [ ! -x "$AAPT2" ]; then
    echo "[!] Error: aapt2 tool not found."
    exit 1
fi
if [ -z "$D8" ] || [ ! -x "$D8" ]; then
    echo "[!] Error: d8 tool not found."
    exit 1
fi
if [ -z "$ZIPALIGN" ] || [ ! -x "$ZIPALIGN" ]; then
    echo "[!] Error: zipalign tool not found."
    exit 1
fi
if [ -z "$APKSIGNER" ] || [ ! -x "$APKSIGNER" ]; then
    echo "[!] Error: apksigner tool not found."
    exit 1
fi

APP_VERSION="${VERSION:-0.1.0}"
APP_VERSION="${APP_VERSION#v}"

echo "=========================================================="
echo "          Building Standalone Android APK for xChat       "
echo " Version:            v${APP_VERSION}"
echo " Android SDK:        ${ANDROID_SDK}"
echo " Build Tools:        ${BUILD_TOOLS_VERSION}"
echo " Platform JAR:       ${ANDROID_JAR}"
echo "=========================================================="

rm -rf "${BUILD_DIR}"
mkdir -p "${BUILD_DIR}/gen" "${BUILD_DIR}/classes" "${BUILD_DIR}/dex" "${DIST_DIR}"

# 1. Compile Resources
echo "[*] Step 1/7: Compiling resources with aapt2..."
"${AAPT2}" compile --dir "${ANDROID_DIR}/app/src/main/res" -o "${BUILD_DIR}/compiled_res.zip"

# 2. Link Resources & Manifest
echo "[*] Step 2/7: Linking manifest and assets..."
"${AAPT2}" link \
    -I "${ANDROID_JAR}" \
    "${BUILD_DIR}/compiled_res.zip" \
    --manifest "${ANDROID_DIR}/app/src/main/AndroidManifest.xml" \
    -A "${ANDROID_DIR}/app/src/main/assets" \
    --java "${BUILD_DIR}/gen" \
    -o "${BUILD_DIR}/app-unaligned.apk" \
    --auto-add-overlay

# 3. Compile Java Source Code
echo "[*] Step 3/7: Compiling Java sources..."
find "${BUILD_DIR}/gen" -name "*.java" > "${BUILD_DIR}/sources.txt"
find "${ANDROID_DIR}/app/src/main/java" -name "*.java" >> "${BUILD_DIR}/sources.txt"

javac -source 8 -target 8 \
    -cp "${ANDROID_JAR}" \
    -d "${BUILD_DIR}/classes" \
    @"${BUILD_DIR}/sources.txt"

# 4. Convert Bytecode to Dalvik DEX
echo "[*] Step 4/7: Converting to DEX bytecode via d8..."
find "${BUILD_DIR}/classes" -name "*.class" > "${BUILD_DIR}/classes.txt"
"${D8}" --lib "${ANDROID_JAR}" --min-api 21 \
    --output "${BUILD_DIR}/dex" \
    $(cat "${BUILD_DIR}/classes.txt")

# 5. Pack DEX into APK
echo "[*] Step 5/7: Packing classes.dex into APK..."
(cd "${BUILD_DIR}/dex" && zip -u "${BUILD_DIR}/app-unaligned.apk" classes.dex)

# 6. Zipalign APK (4-byte alignment, force overwrite)
echo "[*] Step 6/7: Aligning APK with zipalign..."
"${ZIPALIGN}" -f -v -p 4 "${BUILD_DIR}/app-unaligned.apk" "${BUILD_DIR}/xchat-aligned.apk"

# 7. Sign APK with apksigner
echo "[*] Step 7/7: Signing APK with apksigner..."
KEYSTORE="${ANDROID_DIR}/release.keystore"
if [ ! -f "$KEYSTORE" ]; then
    KEYSTORE="${ANDROID_DIR}/debug.keystore"
fi
if [ ! -f "$KEYSTORE" ]; then
    echo "[*] Creating production-compatible release signing keystore..."
    keytool -genkeypair -v -keystore "$KEYSTORE" \
        -alias xchat \
        -keyalg RSA \
        -keysize 2048 \
        -validity 10000 \
        -storepass xchatpass \
        -keypass xchatpass \
        -dname "CN=xChat Release, OU=Mobile Security, O=xChat Network, L=Global, S=Global, C=XX"
fi

APK_OUT="${DIST_DIR}/xchat-v${APP_VERSION}.apk"
GENERIC_APK="${DIST_DIR}/xchat.apk"

"${APKSIGNER}" sign \
    --ks "$KEYSTORE" \
    --ks-pass pass:xchatpass \
    --key-pass pass:xchatpass \
    --out "${APK_OUT}" \
    "${BUILD_DIR}/xchat-aligned.apk"

# Also create generic xchat.apk
cp "${APK_OUT}" "${GENERIC_APK}"

# Verify signature
echo "[*] Verifying signed APK..."
"${APKSIGNER}" verify --verbose "${APK_OUT}"

generate_sha256() {
    local target="$1"
    local filename
    filename="$(basename "$target")"
    local dir
    dir="$(dirname "$target")"
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$dir" && sha256sum "$filename" > "${filename}.sha256")
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$dir" && shasum -a 256 "$filename" > "${filename}.sha256")
    fi
}

generate_sha256 "${APK_OUT}"
generate_sha256 "${GENERIC_APK}"

echo "=========================================================="
echo " [✓] Android APK Built Successfully!"
echo " Output: ${APK_OUT}"
echo " Size:   $(ls -lh "${APK_OUT}" | awk '{print $5}')"
echo " SHA256: $(cat "${DIST_DIR}/xchat-v${APP_VERSION}.apk.sha256")"
echo "=========================================================="
