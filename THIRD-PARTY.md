# Third-party material

Familiar is its own code. It is inspired by [BeipMU](https://github.com/BeipDev/BeipMU)
and imports BeipMU's files, but it isn't a port of BeipMU and isn't affiliated with its developers.
"BeipMU" is the name of their project.

These files come from BeipMU, which is under the MIT licence (Copyright (c) 1998-2019 Bennet; the full
text is in `crates/config/LICENSE-BeipMU`):

- `crates/config/Worlds.xml`: the built-in directory of public worlds.
- `crates/import/tests/data/SampleConfig.txt`, `Fluff.atlas` and `Arx_Map_by_Precisi.atlas`: test
  data for the BeipMU importer.

Familiar's own licence is in `LICENSE`. Rust dependencies keep their own licences; `cargo about` or
`cargo license` lists them.

The Windows zip and installer also contain the libraries Familiar runs on there, as built by the
[MSYS2](https://www.msys2.org) project: GTK 4, libadwaita, GtkSourceView, libspelling, Enchant,
Hunspell and its English dictionaries, ICU, Pango, Cairo, HarfBuzz, GLib, librsvg, the Adwaita icon
theme and the libraries those depend on. Most are under the LGPL; each one's licence is in the
`licenses` folder of the Windows files.
