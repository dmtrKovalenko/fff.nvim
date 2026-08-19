SHELL := bash
.SHELLFLAGS := -o pipefail -ec

LUA_FILES := lua/fff_plus/*.lua lua/fff_plus/pickers/*.lua plugin/fff_plus.lua tests/*.lua
UNIT_TESTS := tests/test_fff_plus_extension.lua tests/test_picker_core.lua tests/test_async.lua tests/test_actions.lua tests/test_matcher.lua tests/test_experience.lua

.PHONY: all test test-lua test-integration lint format format-check

all: format-check lint test

test: test-lua

test-lua:
	@set -e; for test in $(UNIT_TESTS); do \
		NVIM_LOG_FILE=/tmp/fff-plus-nvim.log nvim --headless -u tests/minimal_init.lua -l "$$test"; \
	done

test-integration:
	@test -n "$(FFF_UPSTREAM)" || (echo "FFF_UPSTREAM must point to an upstream fff checkout" && exit 1)
	FFF_UPSTREAM="$(FFF_UPSTREAM)" NVIM_LOG_FILE=/tmp/fff-plus-nvim.log \
		nvim --headless -u tests/integration_init.lua -l tests/test_real_upstream.lua

lint:
	@for file in $(LUA_FILES); do \
		luajit -bl "$$file" "/tmp/$$(basename "$$file").out"; \
	done

format:
	stylua lua plugin tests

format-check:
	stylua --check lua plugin tests
