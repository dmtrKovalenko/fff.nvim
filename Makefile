PLENARY_DIR ?= ../plenary.nvim

PREFIX ?= /usr/local
LIBDIR ?= $(PREFIX)/lib
INCLUDEDIR ?= $(PREFIX)/include

.PHONY: build build-c-lib install uninstall test test-rust test-lua test-version test-bun test-node prepare-bun prepare-node set-npm-version header

all: format test lint

build:
	cargo build --release --features zlob

build-c-lib:
	cargo build --release -p fff-c --features zlob

header:
	cbindgen --config crates/fff-c/cbindgen.toml --crate fff-c --output crates/fff-c/include/fff.h

# Install the C library and header under $(PREFIX) (default /usr/local).
# Override PREFIX for user-local installs, e.g. `make install PREFIX=$$HOME/.local`.
# DESTDIR is honoured for packagers.
install: build-c-lib
	install -d $(DESTDIR)$(LIBDIR)
	install -d $(DESTDIR)$(INCLUDEDIR)
	install -m 0644 crates/fff-c/include/fff.h $(DESTDIR)$(INCLUDEDIR)/fff.h
	@if [ -f target/release/libfff_c.dylib ]; then \
		install -m 0755 target/release/libfff_c.dylib $(DESTDIR)$(LIBDIR)/libfff_c.dylib; \
		echo "Installed $(DESTDIR)$(LIBDIR)/libfff_c.dylib"; \
	fi
	@if [ -f target/release/libfff_c.so ]; then \
		install -m 0755 target/release/libfff_c.so $(DESTDIR)$(LIBDIR)/libfff_c.so; \
		echo "Installed $(DESTDIR)$(LIBDIR)/libfff_c.so"; \
	fi
	@if [ -f target/release/fff_c.dll ]; then \
		install -m 0755 target/release/fff_c.dll $(DESTDIR)$(LIBDIR)/fff_c.dll; \
		echo "Installed $(DESTDIR)$(LIBDIR)/fff_c.dll"; \
	fi
	@echo "Installed header $(DESTDIR)$(INCLUDEDIR)/fff.h"

uninstall:
	rm -f $(DESTDIR)$(LIBDIR)/libfff_c.dylib
	rm -f $(DESTDIR)$(LIBDIR)/libfff_c.so
	rm -f $(DESTDIR)$(LIBDIR)/fff_c.dll
	rm -f $(DESTDIR)$(INCLUDEDIR)/fff.h
	@echo "Removed fff-c from $(DESTDIR)$(PREFIX)"

test-setup:
	@if [ ! -d "$(PLENARY_DIR)" ]; then \
		echo "Cloning plenary.nvim..."; \
		git clone --depth 1 https://github.com/nvim-lua/plenary.nvim $(PLENARY_DIR); \
	fi

test-rust:
	cargo test --workspace --features zlob --exclude fff-nvim

test-lua: test-setup build
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedDirectory tests/ {minimal_init = 'tests/minimal_init.lua'}" 2>&1

test-version: test-setup
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedFile tests/version_spec.lua" 2>&1

prepare-bun: build
	mkdir -p packages/fff-bun/bin
	cp target/release/libfff_c.dylib packages/fff-bun/bin/ 2>/dev/null; \
	cp target/release/libfff_c.so packages/fff-bun/bin/ 2>/dev/null; \
	cp target/release/fff_c.dll packages/fff-bun/bin/ 2>/dev/null; \
	true

prepare-node: build
	mkdir -p packages/fff-node/bin
	cp target/release/libfff_c.dylib packages/fff-node/bin/ 2>/dev/null; \
	cp target/release/libfff_c.so packages/fff-node/bin/ 2>/dev/null; \
	cp target/release/fff_c.dll packages/fff-node/bin/ 2>/dev/null; \
	true

test-bun: prepare-bun
	cd packages/fff-bun && bun test src/

test-node: prepare-node
	cd packages/fff-node && npm run build && node test/e2e.mjs

test: test-rust test-lua test-version test-bun test-node

# Update version in a package.json, including optionalDependencies.
# Usage: make set-npm-version PKG=packages/fff-bun VERSION=1.0.0-nightly.abc1234
set-npm-version:
	@test -n "$(PKG)" || (echo "PKG is required" && exit 1)
	@test -n "$(VERSION)" || (echo "VERSION is required" && exit 1)
	node -e " \
		const fs = require('fs'); \
		const pkg = JSON.parse(fs.readFileSync('$(PKG)/package.json', 'utf8')); \
		pkg.version = '$(VERSION)'; \
		if (pkg.optionalDependencies) { \
			for (const dep of Object.keys(pkg.optionalDependencies)) { \
				pkg.optionalDependencies[dep] = '$(VERSION)'; \
			} \
		} \
		fs.writeFileSync('$(PKG)/package.json', JSON.stringify(pkg, null, 2) + '\n'); \
	"
	@echo "Set $(PKG) to $(VERSION)"

format-rust:
	cargo fmt --all
format-lua:
	stylua .
format-ts:
	bun format

format: format-rust format-lua format-ts

lint-rust:
	cargo clippy --workspace --features zlob -- -D warnings
lint-lua:
	 ~/.luarocks/bin/luacheck .
lint-ts:
	bun lint

lint: lint-rust lint-lua lint-ts

check: format lint

CRATES_TO_PUBLISH= fff-grep fff-query-parser fff-search

publish-crates:
	@test -n "$(V)" || (echo "V is required. Usage: make publish-crates V=0.2.0" && exit 1)
	cargo install cargo-edit
	cargo set-version $(V) || exit 1;
	@for crate in $(CRATES_TO_PUBLISH); do \
		cargo publish -p $$crate --allow-dirty $$(if [ -n "$$CI" ]; then echo "--no-verify"; fi) || exit 1; \
	done
