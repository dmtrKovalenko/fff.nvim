SHELL := bash
.SHELLFLAGS := -o pipefail -ec

LUA_FILES := lua/fff_plus/init.lua lua/fff_plus/git_utils.lua lua/fff_plus/pickers/*.lua plugin/fff_plus.lua tests/test_fff_plus_extension.lua

.PHONY: all test test-lua lint format format-check

all: format-check lint test

test: test-lua

test-lua:
	nvim --headless -u tests/minimal_init.lua -l tests/test_fff_plus_extension.lua

lint:
	@for file in $(LUA_FILES); do \
		luajit -bl "$$file" "/tmp/$$(basename "$$file").out"; \
	done

format:
	stylua lua plugin tests/test_fff_plus_extension.lua

format-check:
	stylua --check lua plugin tests/test_fff_plus_extension.lua
