---@class fff.fuzzy
local M = {}

-- Try to load the Rust module
local ok, rust_module = pcall(require, 'fff.rust')
if not ok then error('Failed to load fff.rust module: ' .. rust_module) end

-- export all functions from the Rust module
M.init_db = rust_module.init_db
M.destroy_db = rust_module.destroy_db
M.access = rust_module.access
M.set_provider_items = rust_module.set_provider_items
M.fuzzy = rust_module.fuzzy
M.fuzzy_matched_indices = rust_module.fuzzy_matched_indices
M.get_keyword_range = rust_module.get_keyword_range
M.guess_edit_range = rust_module.guess_edit_range
M.get_words = rust_module.get_words
M.init_file_picker = rust_module.init_file_picker
M.restart_index_in_path = rust_module.restart_index_in_path
M.scan_files = rust_module.scan_files
M.get_cached_files = rust_module.get_cached_files
M.fuzzy_search_files = rust_module.fuzzy_search_files
M.track_access = rust_module.track_access
M.add_file = rust_module.add_file
M.remove_file = rust_module.remove_file
M.cancel_scan = rust_module.cancel_scan
M.get_scan_progress = rust_module.get_scan_progress
M.is_scanning = rust_module.is_scanning
M.refresh_git_status = rust_module.refresh_git_status
M.update_single_file_frecency = rust_module.update_single_file_frecency
M.stop_background_monitor = rust_module.stop_background_monitor
M.cleanup_file_picker = rust_module.cleanup_file_picker
M.init_tracing = rust_module.init_tracing
M.wait_for_initial_scan = rust_module.wait_for_initial_scan

return M
