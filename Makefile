PLENARY_DIR ?= ../plenary.nvim

.PHONY: build test test-rust test-lua test-setup

build:
	cargo build --release

test-setup:
	@if [ ! -d "$(PLENARY_DIR)" ]; then \
		echo "Cloning plenary.nvim..."; \
		git clone --depth 1 https://github.com/nvim-lua/plenary.nvim $(PLENARY_DIR); \
	fi

test-rust:
	cargo test --verbose --workspace --exclude fff-nvim

test-lua: test-setup build
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedFile tests/fff_core_spec.lua" 2>&1

test: test-rust test-lua

format-rust:
	cargo fmt --all
format-lua:
	stylua .

format: format-rust format-lua
