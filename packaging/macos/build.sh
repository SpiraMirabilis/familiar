#!/bin/bash
# Builds dist/Familiar.app (and dist/Familiar-VERSION-macos.zip): the Rust engine as a static
# library, the Swift app on top of it, and the bundle around them.
#
#   packaging/macos/build.sh                 for this Mac's processor, signed ad hoc
#   UNIVERSAL=1 packaging/macos/build.sh     for Apple silicon and Intel
#
# To sign for distribution, set SIGN_IDENTITY to a Developer ID Application certificate in your
# keychain ("Developer ID Application: Your Name (TEAMID)"). To notarize as well, set
# NOTARY_PROFILE to a profile saved with `xcrun notarytool store-credentials`.
set -euo pipefail

cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUNDLE_ID=${BUNDLE_ID:-io.github.SpiraMirabilis.Familiar}
SIGN_IDENTITY=${SIGN_IDENTITY:--}
APP=dist/Familiar.app
# The oldest macOS the app runs on. Keep in step with macos/Package.swift and Info.plist.
export MACOSX_DEPLOYMENT_TARGET=14.0

if [ -n "${UNIVERSAL:-}" ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    cargo build --release --locked -p familiar-bridge --target aarch64-apple-darwin --target x86_64-apple-darwin
    mkdir -p target/universal/release
    lipo -create -output target/universal/release/libfamiliar_bridge.a \
        target/aarch64-apple-darwin/release/libfamiliar_bridge.a target/x86_64-apple-darwin/release/libfamiliar_bridge.a
    export FAMILIAR_RUST_LIB_DIR="$PWD/target/universal/release"
    ARCHS=(--arch arm64 --arch x86_64)
else
    cargo build --release --locked -p familiar-bridge
    ARCHS=()
fi

swift build --package-path macos -c release ${ARCHS[@]+"${ARCHS[@]}"}
BINARY="$(swift build --package-path macos -c release ${ARCHS[@]+"${ARCHS[@]}"} --show-bin-path)/Familiar"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BINARY" "$APP/Contents/MacOS/Familiar"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp packaging/macos/Familiar.icns "$APP/Contents/Resources/"
cp LICENSE THIRD-PARTY.md crates/config/LICENSE-BeipMU "$APP/Contents/Resources/"

# The hardened runtime is what notarization asks for. The app needs no entitlements: it isn't
# sandboxed, and nothing in it generates code at run time.
codesign --force --timestamp=none --options runtime --sign "$SIGN_IDENTITY" "$APP" 2>/dev/null \
    || codesign --force --options runtime --sign "$SIGN_IDENTITY" "$APP"
if [ "$SIGN_IDENTITY" != "-" ]; then
    codesign --force --timestamp --options runtime --sign "$SIGN_IDENTITY" "$APP"
fi
codesign --verify --strict "$APP"

ZIP="dist/Familiar-$VERSION-macos.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

if [ -n "${NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$APP"
    # The zip has to hold the stapled app.
    rm -f "$ZIP"
    ditto -c -k --keepParent "$APP" "$ZIP"
fi

echo "Built $APP and $ZIP"
