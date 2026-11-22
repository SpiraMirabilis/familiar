#!/usr/bin/env bash
# Builds Familiar for Windows and collects everything it needs to run on a PC that has never
# heard of GTK. Run from the repository root in an MSYS2 UCRT64 shell:
#
#   pacman -S --needed zip mingw-w64-ucrt-x86_64-{gtk4,libadwaita,gtksourceview5,libspelling,librsvg,pkgconf,gcc,rust,hunspell-en,adwaita-icon-theme,ntldd}
#   packaging/windows/bundle.sh
#
# The result is dist/familiar/ (run Familiar.cmd, or bin/familiar.exe) and
# dist/familiar-<version>-x86_64-windows.zip.
set -euo pipefail

PREFIX=${MINGW_PREFIX:-/ucrt64}
OUT=dist/familiar
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

# Web views need WebKitGTK, which doesn't exist for Windows.
cargo build --release --locked -p familiar-gtk --no-default-features --features spelling
cargo build --release --locked -p familiar-tui

# Emptied rather than removed: Windows won't delete a folder something has open.
mkdir -p "$OUT"
find "$OUT" -mindepth 1 -delete
mkdir -p "$OUT/bin"
cp target/release/familiar.exe "$OUT/bin/"
# The terminal client needs none of the rest, so it sits at the top where it's easy to find.
cp target/release/familiar-tui.exe "$OUT/"
printf '@start "" "%%~dp0bin\\familiar.exe" %%*\r\n' > "$OUT/Familiar.cmd"

# GTK and the libraries under it find their data relative to a bin folder holding their DLLs
# (some, like the spell checker, only that way), so everything keeps the layout it has under
# $PREFIX.
copy() { # copy <path under $PREFIX>...
    for path in "$@"; do
        mkdir -p "$OUT/$(dirname "$path")"
        cp -r "$PREFIX/$path" "$OUT/$path"
    done
}
copy lib/gdk-pixbuf-2.0
# Spelling: just the Hunspell provider and English. Other dictionaries can be dropped into
# share/hunspell by anyone who wants them.
copy lib/enchant-2/enchant_hunspell.dll
copy share/glib-2.0/schemas/gschemas.compiled
copy share/gtksourceview-5
copy share/icons/Adwaita share/icons/hicolor/index.theme
for dictionary in en_US en_GB; do copy "share/hunspell/$dictionary.aff" "share/hunspell/$dictionary.dic"; done
# For GApplication (one running copy, opening a second window in it).
cp "$PREFIX/bin/gdbus.exe" "$OUT/bin/"
find "$OUT/lib" -name '*.a' -delete

# Every DLL the programs and plugins load, and theirs in turn. System DLLs stay where they are.
find "$OUT" \( -name '*.exe' -o -name '*.dll' \) -print0 \
    | xargs -0 ntldd -R 2>/dev/null \
    | awk '$2 == "=>" { print $3 }' | tr '\\' '/' | grep -i "/$(basename "$PREFIX")/bin/" | sort -u \
    | while read -r dll; do cp -n "$(cygpath -u "$dll")" "$OUT/bin/"; done

# The licences of what was just bundled.
mkdir -p "$OUT/licenses"
cp LICENSE "$OUT/licenses/Familiar.txt"
cp THIRD-PARTY.md "$OUT/licenses/"
for dll in "$OUT"/bin/*.dll; do pacman -Qqo "$PREFIX/bin/$(basename "$dll")" 2>/dev/null || true; done | sort -u | while read -r package; do
    name=${package#mingw-w64-ucrt-x86_64-}
    if [ -d "$PREFIX/share/licenses/$name" ]; then
        cp -r "$PREFIX/share/licenses/$name" "$OUT/licenses/$name"
    fi
done

(cd dist && rm -f "familiar-$VERSION-x86_64-windows.zip" && zip -qr "familiar-$VERSION-x86_64-windows.zip" familiar)
du -sh "$OUT" "dist/familiar-$VERSION-x86_64-windows.zip"
