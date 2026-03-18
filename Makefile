PLENARY_DIR ?= ../plenary.nvim

.PHONY: build test test-rust test-lua test-bun test-node prepare-bun prepare-node set-npm-version header

build:
	cargo build --release --features zlob

header:
	cbindgen --config crates/fff-c/cbindgen.toml --crate fff-c --output crates/fff-c/include/fff.h

test-setup:
	@if [ ! -d "$(PLENARY_DIR)" ]; then \
		echo "Cloning plenary.nvim..."; \
		git clone --depth 1 https://github.com/nvim-lua/plenary.nvim $(PLENARY_DIR); \
	fi

test-rust:
	cargo test --workspace --features zlob --exclude fff-nvim

test-lua: test-setup build
	nvim --headless -u tests/minimal_init.lua \
		-c "PlenaryBustedFile tests/fff_core_spec.lua" 2>&1

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

test: test-rust test-lua test-bun test-node

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
