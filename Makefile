PLENARY_DIR ?= ../plenary.nvim

.PHONY: build test test-rust test-lua test-bun test-setup prepare-bun

build:
	cargo build --release

test-setup:
	@if [ ! -d "$(PLENARY_DIR)" ]; then \
		echo "Cloning plenary.nvim..."; \
		git clone --depth 1 https://github.com/nvim-lua/plenary.nvim $(PLENARY_DIR); \
	fi

test-rust:
	cargo test --workspace

test-lua: test-setup build
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedFile tests/fff_core_spec.lua" 2>&1

prepare-bun: build
	mkdir -p packages/fff-bun/bin
	cp target/release/libfff_c.dylib packages/fff-bun/bin/ 2>/dev/null; \
	cp target/release/libfff_c.so packages/fff-bun/bin/ 2>/dev/null; \
	cp target/release/fff_c.dll packages/fff-bun/bin/ 2>/dev/null; \
	true

test-bun: prepare-bun
	cd packages/fff-bun && bun test src/

test: test-rust test-lua test-bun

format-rust:
	cargo fmt --all
format-lua:
	stylua .

format: format-rust format-lua
