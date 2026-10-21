PREFIX ?= $(HOME)/.local
BIN := src-tauri/target/release/pipewire-dj

.PHONY: build install uninstall test dev

build:
	npm install --no-audit --no-fund
	npx tauri build --no-bundle

test:
	cd src-tauri && cargo test

dev:
	npx tauri dev

# The .desktop file's basename is the app id the desktop portal needs for global shortcuts.
install: build
	install -Dm755 $(BIN) $(PREFIX)/bin/pipewire-dj
	install -Dm644 packaging/dev.pipewire_dj.PipeWireDJ.desktop $(PREFIX)/share/applications/dev.pipewire_dj.PipeWireDJ.desktop
	-update-desktop-database $(PREFIX)/share/applications 2>/dev/null

uninstall:
	rm -f $(PREFIX)/bin/pipewire-dj $(PREFIX)/share/applications/dev.pipewire_dj.PipeWireDJ.desktop
