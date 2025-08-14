---@module "state"
local M = {}

---@class State
---@field initialized boolean -- if the file picker has been initialized
---@field last_scan_time number -- the last time a scan of the file system was performed
---@field config Config -- user config
---@field last_search_result SearchResult? -- the last search result

---@class ConfigPreviewFiletypeSettings
---@field wrap_lines boolean? Wrap long lines in preview for this filetype
---@field tail_lines integer? Number of tail lines to show (e.g. for logs)

---@class ConfigPreview
---@field enabled boolean Enable preview pane
---@field width number Preview width as fraction of window
---@field max_lines integer Maximum lines to load in preview
---@field max_size integer Maximum file size in bytes (e.g. 10MB)
---@field imagemagick_info_format_str string ImageMagick info format string
---@field line_numbers boolean Show line numbers in preview
---@field wrap_lines boolean Wrap long lines in preview
---@field show_file_info boolean Show file info header in preview
---@field binary_file_threshold integer Number of bytes to check for binary detection
---@field filetypes table<string, ConfigPreviewFiletypeSettings> Per-filetype preview settings

---@class ConfigLayout
---@field prompt_position '"top"'|'"bottom"' Position of input prompt
---@field preview_position '"right"'|'"left"' Position of preview pane
---@field preview_width number Width of preview pane
---@field height number Window height as fraction of screen
---@field width number Window width as fraction of screen

---@class ConfigKeyMaps
---@field close string Key to close the UI
---@field select string Key to select item
---@field select_split string Key to select and open in split
---@field select_vsplit string Key to select and open in vertical split
---@field select_tab string Key to select and open in new tab
---@field move_up string[] Keys to move selection up (supports multiple)
---@field move_down string[] Keys to move selection down (supports multiple)
---@field preview_scroll_up string Key to scroll preview up
---@field preview_scroll_down string Key to scroll preview down
---@field toggle_debug string Key to toggle debug scores display

---@class ConfigHL
---@field border string Highlight group for border
---@field normal string Highlight group for normal text
---@field cursor string Highlight group for cursor line
---@field matched string Highlight group for matched text
---@field title string Highlight group for window title
---@field prompt string Highlight group for prompt text
---@field active_file string Highlight group for active file line
---@field frecency string Highlight group for frecency info
---@field debug string Highlight group for debug messages

---@class ConfigFrecency
---@field enabled boolean Enable frecency tracking (file access frequency)
---@field db_path string Path to frecency database file

---@class ConfigLogging
---@field enabled boolean Enable logging
---@field log_file string Log file location
---@field log_level '"debug"'|'"info"'|'"warn"'|'"error"' Logging level

---@class ConfigUI
---@field wrap_paths boolean Wrap long file paths in the list UI
---@field wrap_indent integer Indentation spaces for wrapped paths
---@field max_path_width integer Maximum path width before wrapping

---@class ConfigImagePreview
---@field enabled boolean Enable image preview (requires terminal support)
---@field max_width integer Maximum image width in terminal columns
---@field max_height integer Maximum image height in terminal lines

---@class ConfigIcons
---@field enabled boolean Enable file icons display

---@class ConfigDebug
---@field enabled boolean Enable debug mode
---@field show_scores boolean Show scoring information (toggle with keymap)

---@class FindFileOpts
---@field git_ignore boolean Whether to respect .gitignore files
---@field hidden boolean Whether to show hidden files
---@field git_exclude boolean Whether to respect `.git/info/exclude`
---@field git_global boolean Whether to respect global gitignore (`core.excludesFile`)
---@field follow_links boolean Whether to follow symbolic links
---@field ignore boolean Whether to respect `.ignore` files

---@class Config user configuration
---@field base_path string Base directory for file indexing
---@field max_results integer Maximum number of search results to display
---@field max_threads integer Maximum number of threads for fuzzy search
---@field prompt string Input prompt symbol
---@field title string Window title
---@field ui_enabled boolean Enable UI (default: true)
---@field width number Window width as fraction of screen
---@field height number Window height as fraction of screen
---@field preview ConfigPreview Preview pane configuration
---@field layout ConfigLayout Layout configuration (alternative to width/height)
---@field keymaps ConfigKeyMaps Key mappings
---@field hl ConfigHL Highlight groups
---@field frecency ConfigFrecency Frecency tracking options
---@field logging ConfigLogging Logging configuration
---@field ui ConfigUI UI appearance options
---@field image_preview ConfigImagePreview Image preview options
---@field icons ConfigIcons File icon display options
---@field debug ConfigDebug Debug options

---@type Config
local default_config = {
  base_path = vim.fn.getcwd(),
  max_results = 100,
  max_threads = 4,
  prompt = '🪿 ',
  title = 'FFF Files',
  ui_enabled = true,
  width = 0.8,
  height = 0.8,
  preview = {
    enabled = true,
    width = 0.5,
    max_lines = 5000,
    max_size = 10 * 1024 * 1024,
    imagemagick_info_format_str = '%m: %wx%h, %[colorspace], %q-bit',
    line_numbers = false,
    wrap_lines = false,
    show_file_info = true,
    binary_file_threshold = 1024,
    filetypes = {
      svg = { wrap_lines = true },
      markdown = { wrap_lines = true },
      text = { wrap_lines = true },
      log = { tail_lines = 100 },
    },
  },
  layout = {
    prompt_position = 'top',
    preview_position = 'right',
    preview_width = 0.4,
    height = 0.8,
    width = 0.8,
  },
  keymaps = {
    close = '<Esc>',
    select = '<CR>',
    select_split = '<C-s>',
    select_vsplit = '<C-v>',
    select_tab = '<C-t>',
    move_up = { '<Up>', '<C-p>' },
    move_down = { '<Down>', '<C-n>' },
    preview_scroll_up = '<C-u>',
    preview_scroll_down = '<C-d>',
    toggle_debug = '<F2>',
  },
  hl = {
    border = 'FloatBorder',
    normal = 'Normal',
    cursor = 'CursorLine',
    matched = 'IncSearch',
    title = 'Title',
    prompt = 'Question',
    active_file = 'Visual',
    frecency = 'Number',
    debug = 'Comment',
  },
  frecency = {
    enabled = true,
    db_path = vim.fn.stdpath('cache') .. '/fff_nvim',
  },
  ui = {
    wrap_paths = true,
    wrap_indent = 2,
    max_path_width = 80,
  },
  logging = {
    enabled = true,
    log_file = vim.fn.stdpath('log') .. '/fff.log',
    log_level = 'info',
  },
  image_preview = {
    enabled = true,
    max_width = 80,
    max_height = 24,
  },
  icons = {
    enabled = true,
  },
  debug = {
    enabled = false,
    show_scores = false,
  },
}

-- State
---@type State
M = {
  initialized = false,
  last_scan_time = 0,
  config = default_config,
}

return M
