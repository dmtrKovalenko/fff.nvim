PLENARY_DIR ?= ../plenary.nvim

.PHONY: build test test-rust test-lua test-bun test-setup prepare-bun

build:
	cargo build --release --features zlob

test-setup:
	@if [ ! -d "$(PLENARY_DIR)" ]; then \
		echo "Cloning plenary.nvim..."; \
		git clone --depth 1 https://github.com/nvim-lua/plenary.nvim $(PLENARY_DIR); \
	fi

test-rust:
	cargo test --workspace --features zlob

test-lua: test-setup build
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedFile tests/fff_core_spec.lua" 2>&1

prepare-bun: build
	mkdir -p packages/fff-bun/bin
	cp target/release/libfff_c.dylib packages/fff-bun/bin/ 2>/dev/null; \
	cp target/release/libfff_c.so packages/fff-bun/bin/ 2>/dev/null; \
	cp target/release/fff_c.dll packages/fff-bun/bin/ 2>/dev/null; \
	true
	@# Re-sign on macOS: cp can invalidate ad-hoc code signatures
	@if [ "$$(uname)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then \
		codesign --sign - packages/fff-bun/bin/libfff_c.dylib 2>/dev/null || true; \
	fi

test-bun: prepare-bun
	cd packages/fff-bun && bun test src/

test: test-rust test-lua test-bun

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
