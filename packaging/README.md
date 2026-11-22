# Packaging Familiar

Everything installs through the top-level `Makefile`:
`make && make install DESTDIR=... PREFIX=/usr`. It installs `familiar` and `familiar-tui`, two desktop
entries, the AppStream metainfo, the icon, documentation and licences (`LICENSEDIR` moves the
licences, e.g. to `/usr/share/licenses/$pkgname`).

## Releasing

1. Update the version in `Cargo.toml` (workspace), `CHANGELOG.md`, the `<releases>` in
   `data/io.github.SpiraMirabilis.Familiar.metainfo.xml` and `pkgver` in `aur/familiar/PKGBUILD`.
2. `cargo test --workspace`, then tag `vX.Y.Z` and push the tag. The release workflow builds a
   tarball and a Flatpak bundle and attaches them to a draft GitHub release.
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
