PLENARY_DIR ?= ../plenary.nvim
MINI_DIR ?= ../mini.nvim

PREFIX ?= /usr/local
LIBDIR ?= $(PREFIX)/lib
INCLUDEDIR ?= $(PREFIX)/include

# Compile-time cfg that gates the watcher + git-status fuzz stress test.
STRESS_RUSTFLAGS := --cfg stress
FFF_STRESS_DEFAULT_SEED ?= 0xDEADBEEFCAFEBABE

SHELL := bash
# Order matters: `-c` must be last so bash treats the recipe as the script
# string rather than the literal `-o` / `pipefail` tokens.
.SHELLFLAGS := -o pipefail -ec

.PHONY: build build-c-lib install uninstall test test-rust test-c-smoke test-c-api test-lua test-lua-snap test-version test-bun test-node prepare-bun prepare-node set-npm-version header test-stress test-stress-seeded test-stress-random test-stress-repos test-node-stress sync-js-api sync-js-api-check

all: format test lint

# Single source of truth for the shared FileFinder TS interface lives in
# packages/shared/fff-api.ts. tsc cannot import across a package's
# rootDir and the bun package publishes its raw src/, so the file is copied
# into each package instead of symlinked.
SYNC_API_SRC := packages/shared/fff-api.ts
SYNC_API_TARGETS := packages/fff-node/src/fff-api.ts packages/fff-bun/src/fff-api.ts
SYNC_API_BANNER := // ----------------------------------------------------------------------------\n// GENERATED FILE - DO NOT EDIT.\n// Source of truth: packages/shared/fff-api.ts\n// Run make sync-js-api from the repo root to regenerate.\n// ----------------------------------------------------------------------------\n\n

sync-js-api:
	@for target in $(SYNC_API_TARGETS); do \
		printf '$(SYNC_API_BANNER)' > "$$target"; \
		cat $(SYNC_API_SRC) >> "$$target"; \
		echo "synced: $$target"; \
	done

sync-js-api-check:
	@status=0; \
	for target in $(SYNC_API_TARGETS); do \
		tmp=$$(mktemp); \
		printf '$(SYNC_API_BANNER)' > "$$tmp"; \
		cat $(SYNC_API_SRC) >> "$$tmp"; \
		if ! cmp -s "$$tmp" "$$target"; then \
			echo "out of date: $$target (run make sync-js-api)"; status=1; \
		fi; \
		rm -f "$$tmp"; \
	done; \
	exit $$status

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
	@if [ ! -d "$(MINI_DIR)" ]; then \
		echo "Cloning mini.nvim..."; \
		git clone --depth 1 https://github.com/echasnovski/mini.nvim $(MINI_DIR); \
	fi

test-rust:
	cargo test --workspace --features zlob --exclude fff-nvim

CC ?= cc
CFLAGS ?= -O0 -g -Wall -Wextra -std=c99
TARGET_DIR ?= target/release
SMOKE_BIN := $(TARGET_DIR)/fff_c_smoke
SMOKE_SRC := crates/fff-c/tests/smoke.c
SMOKE_INCLUDE := crates/fff-c/include

test-c-smoke: build-c-lib
	$(CC) $(CFLAGS) -I $(SMOKE_INCLUDE) -L $(TARGET_DIR) \
		-Wl,-rpath,@loader_path/../target/release \
		-Wl,-rpath,$$(pwd)/$(TARGET_DIR) \
		$(SMOKE_SRC) -lfff_c -o $(SMOKE_BIN)
	$(SMOKE_BIN) .

# Alias kept for the `external-tests.yml` workflow naming.
test-c-api: test-c-smoke

# neovim instance swallows internal crashes and doesn't rise the the error exiting silently
# so check the stdout in case the sigsegv coming out of fff was printed (actual regression).
# Output is streamed live via `tee`; pipefail (set above) propagates nvim's exit.
test-lua: test-setup build
	@logfile=$$(mktemp); \
	trap 'rm -f "$$logfile"' EXIT; \
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedDirectory tests/ {minimal_init = 'tests/minimal_init.lua'}" 2>&1 \
		| tee "$$logfile"; \
	if grep -qE "SIG(SEGV|ABRT|BUS|FPE|ILL)" "$$logfile"; then \
		echo ""; \
		echo "FAIL: native crash detected during lua tests"; \
		exit 1; \
	fi

# mini.test reference_screenshot snapshots. Separate runner because mini.test
# spawns child processes and uses its own collector (incompatible with
# PlenaryBustedDirectory). Streams output live via `tee` so failure diffs
# appear as they happen instead of after a long capture-buffered silence.
# `pcall` catches collect-time errors (e.g. parse error in the test file)
# that would otherwise leave headless nvim hanging in its event loop because
# the reporter's `cquit` never fires.
test-lua-snap: test-setup build
	@logfile=$$(mktemp); \
	trap 'rm -f "$$logfile"' EXIT; \
	nvim --headless -u tests/minimal_init.lua \
		-c "lua local ok,err=pcall(require('mini.test').run_file,'tests/picker_ui_snap.lua'); if not ok then io.stderr:write('mini.test failed to load: '..tostring(err)..'\\n'); vim.cmd('cquit 2') end" 2>&1 \
		| tee "$$logfile"; \
	if grep -qE "SIG(SEGV|ABRT|BUS|FPE|ILL)" "$$logfile"; then \
		echo ""; \
		echo "FAIL: native crash detected during snapshot tests"; \
		exit 1; \
	fi

test-version: test-setup
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedFile tests/version_spec.lua" 2>&1

prepare-bun: build sync-js-api
	mkdir -p packages/fff-bun/bin
	cp target/release/libfff_c.dylib packages/fff-bun/bin/ 2>/dev/null || true; \
	cp target/release/libfff_c.so packages/fff-bun/bin/ 2>/dev/null || true; \
	cp target/release/fff_c.dll packages/fff-bun/bin/ 2>/dev/null || true

prepare-node: build sync-js-api
	mkdir -p packages/fff-node/bin
	cp target/release/libfff_c.dylib packages/fff-node/bin/ 2>/dev/null || true; \
	cp target/release/libfff_c.so packages/fff-node/bin/ 2>/dev/null || true; \
	cp target/release/fff_c.dll packages/fff-node/bin/ 2>/dev/null || true

test-bun: prepare-bun
	cd packages/fff-bun && bun test src/
	cd packages/pi-fff && bun test test/

test-node: prepare-node
	cd packages/fff-node && npm run build && node test/e2e.mjs

test-js: test-bun test-node

# Bug pinning stress test script over fff-node for issue #515
# Just keep it untouched because it's good enough + some stress for SDK
FFF_STRESS_ITERS ?= 50
test-node-stress: prepare-node
	cd packages/fff-node && npm run build && \
		FFF_STRESS_ITERS=$(FFF_STRESS_ITERS) node test/stress-515.mjs

test: test-rust test-lua test-lua-snap test-version test-bun test-node test-node-stress

test-stress-seeded:
	FFF_STRESS_SEED="$${FFF_STRESS_SEED:-$(FFF_STRESS_DEFAULT_SEED)}" \
	RUSTFLAGS="$(STRESS_RUSTFLAGS)" \
	cargo test --release \
		-p fff-search \
		--test fuzz_git_watcher_stress \
		--features zlob \
		-- --nocapture stress_seeded

test-stress-random:
	RUSTFLAGS="$(STRESS_RUSTFLAGS)" \
	cargo test --release \
		-p fff-search \
		--test fuzz_git_watcher_stress \
		--features zlob \
		-- --nocapture stress_random

test-stress-repos:
	RUSTFLAGS="$(STRESS_RUSTFLAGS)" \
	cargo test --release \
		-p fff-search \
		--test fuzz_real_repos \
		--features zlob \
		-- --nocapture

test-stress: test-stress-seeded test-stress-random test-stress-repos

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
	cargo install cargo-edit --force --locked
	cargo set-version $(V) || exit 1;
	@for crate in $(CRATES_TO_PUBLISH); do \
		cargo publish -p $$crate --allow-dirty $$(if [ -n "$$CI" ]; then echo "--no-verify"; fi) || exit 1; \
	done
