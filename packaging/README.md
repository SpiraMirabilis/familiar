# Packaging Familiar

Everything installs through the top-level `Makefile`:
`make && make install DESTDIR=... PREFIX=/usr`. It installs `familiar` and `familiar-tui`, two desktop
entries, the AppStream metainfo, the icon, documentation and licences (`LICENSEDIR` moves the
licences, e.g. to `/usr/share/licenses/$pkgname`).

## Releasing

1. Update the version in `Cargo.toml` (workspace), `CHANGELOG.md`, the `<releases>` in
   `data/io.github.SpiraMirabilis.Familiar.metainfo.xml` and `pkgver` in `aur/familiar/PKGBUILD`.
2. `cargo test --workspace`, then tag `vX.Y.Z` and push the tag. The release workflow builds a
   tarball, a Flatpak bundle, the Windows files (zip, installer, terminal client) and the macOS
   ones (the app as a zip, and the terminal client), and attaches them to a draft GitHub release.
3. AUR: put the tarball's sha256 in `aur/familiar/PKGBUILD`, run `makepkg --printsrcinfo > .SRCINFO`
   in each AUR folder, and push them to the AUR's git repositories.
4. crates.io: `cargo publish --workspace` (familiar-cli isn't published).

## AUR (`aur/`)

`familiar` builds a release tarball; `familiar-git` builds the latest code. Both need
`options=('!lto')`: makepkg's LTO turns the C parts (ring, QuickJS) into objects Rust can't link.
Check with `makepkg -f` and `namcap` on the PKGBUILD and the package.

## Flatpak (`flatpak/`)

The manifest builds against the GNOME runtime and adds libspelling, which the runtime lacks. Crates
are vendored from `cargo-sources.json`; regenerate it whenever `Cargo.lock` changes:

```sh
python3 packaging/flatpak/cargo-sources.py
flatpak-builder --user --force-clean --repo=repo build-dir packaging/flatpak/io.github.SpiraMirabilis.Familiar.yml
flatpak build-bundle repo familiar.flatpak io.github.SpiraMirabilis.Familiar
```

For Flathub, change the `dir` source to a `git` source pinned to a release tag, and add
screenshots to the metainfo (Flathub requires them).

Inside the Flatpak, settings live in `~/.var/app/io.github.SpiraMirabilis.Familiar/config/familiar/`,
separate from a native install. Text to speech uses the host's speech-dispatcher socket.

## Windows (`windows/`)

`build.ps1` builds the engine as a DLL (`familiar_bridge.dll`) and the terminal client with Cargo,
then publishes the app in [`windows/`](../windows) (C#, WinUI 3) on top of them into
`dist\familiar\`, and zips it. It needs Rust and the .NET 10 SDK; Visual Studio isn't needed. The
folder is self-contained: .NET and the Windows App SDK are in it, so a PC needs nothing installed
first. Web views use WebView2, which Windows already has.

`familiar.iss` turns that folder into an installer with [Inno Setup](https://jrsoftware.org/isinfo.php):
`iscc /DAppVersion=X.Y.Z packaging\windows\familiar.iss`. It installs per user without
administrator rights by default. `familiar.ico` is generated from the SVG icon:

```sh
magick -background none -density 384 data/io.github.SpiraMirabilis.Familiar.svg \
  -define icon:auto-resize=256,128,64,48,32,24,16 packaging/windows/familiar.ico
```

The script stops if the engine's DLL needs MinGW's runtime libraries, which only the PC that built
it would have. The files aren't code-signed, so Windows SmartScreen warns on first run.

## macOS (`macos/`)

The Mac app isn't the GTK client: it's a Swift app (the package in the top-level `macos/` folder)
on top of the engine, which `crates/bridge` builds as a static library. `build.sh` builds both,
puts them in `dist/Familiar.app` and zips it. `UNIVERSAL=1` builds for Apple silicon and Intel.
Nothing is bundled besides the one binary and the icon, since everything else is the system's.

```sh
packaging/macos/build.sh                                   # signed ad hoc: runs on this Mac
SIGN_IDENTITY="Developer ID Application: Name (TEAMID)" \
  NOTARY_PROFILE=familiar UNIVERSAL=1 packaging/macos/build.sh   # for distribution
```

`SIGN_IDENTITY` names a Developer ID Application certificate in the keychain, and
`NOTARY_PROFILE` a profile saved once with
`xcrun notarytool store-credentials familiar --apple-id … --team-id … --password …` (an
app-specific password). The app uses the hardened runtime and needs no entitlements. The release
workflow does the same when the repository has the secrets `MACOS_CERTIFICATE` (the certificate
and its key as a base64 .p12), `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGN_IDENTITY`, `APPLE_ID`,
`APPLE_TEAM_ID` and `APPLE_APP_PASSWORD`; without them the app is signed ad hoc and macOS warns
when it's first opened. The terminal client's binary isn't signed either way.

`Familiar.icns` is generated from the SVG icon by `swift packaging/macos/make-icon.swift`.

While working on the app, `cargo build --release -p familiar-bridge && swift run --package-path macos`
runs it without a bundle (notifications need the bundle). The Mac App Store would need the app
sandboxed, which it isn't: it reads scripts and sounds from wherever the config says.
