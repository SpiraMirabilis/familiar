# Builds and installs Familiar. Packagers: make && make install DESTDIR="$pkgdir" PREFIX=/usr
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
# Packages whose name isn't familiar (e.g. familiar-git) set this to their own folder.
LICENSEDIR ?= $(DATADIR)/licenses/familiar
CARGO ?= cargo
CARGOFLAGS ?= --locked
APP_ID = io.github.SpiraMirabilis.Familiar
# Binaries: target/release after building, or bin/ in a release tarball.
TARGET ?= $(if $(wildcard bin/familiar),bin,target/release)

.PHONY: all build install uninstall check clean

all: build

build:
	$(CARGO) build --release $(CARGOFLAGS) -p familiar-gtk -p familiar-tui

check:
	$(CARGO) test $(CARGOFLAGS) --workspace

install:
	install -Dm755 $(TARGET)/familiar $(DESTDIR)$(BINDIR)/familiar
	install -Dm755 $(TARGET)/familiar-tui $(DESTDIR)$(BINDIR)/familiar-tui
	install -Dm644 data/$(APP_ID).desktop $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop
	install -Dm644 data/$(APP_ID).Terminal.desktop $(DESTDIR)$(DATADIR)/applications/$(APP_ID).Terminal.desktop
	install -Dm644 data/$(APP_ID).metainfo.xml $(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	install -Dm644 data/$(APP_ID).svg $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg
	install -Dm644 README.md $(DESTDIR)$(DATADIR)/doc/familiar/README.md
	install -Dm644 examples/startup.js $(DESTDIR)$(DATADIR)/doc/familiar/examples/startup.js
	install -Dm644 LICENSE $(DESTDIR)$(LICENSEDIR)/LICENSE
	install -Dm644 THIRD-PARTY.md $(DESTDIR)$(LICENSEDIR)/THIRD-PARTY.md
	install -Dm644 crates/config/LICENSE-BeipMU $(DESTDIR)$(LICENSEDIR)/LICENSE-BeipMU

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/familiar $(DESTDIR)$(BINDIR)/familiar-tui
	rm -f $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop $(DESTDIR)$(DATADIR)/applications/$(APP_ID).Terminal.desktop
	rm -f $(DESTDIR)$(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	rm -f $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg
	rm -rf $(DESTDIR)$(DATADIR)/doc/familiar $(DESTDIR)$(LICENSEDIR)

clean:
	$(CARGO) clean
