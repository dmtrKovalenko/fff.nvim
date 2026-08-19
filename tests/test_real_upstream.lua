local function assert_module(name)
  package.loaded[name] = nil
  local ok, module = pcall(require, name)
  assert(ok, string.format('expected real upstream module %s to load: %s', name, tostring(module)))
  return module
end

local upstream = assert_module('fff')
assert(type(upstream.find_files) == 'function', 'real upstream fff.find_files should exist')

for _, name in ipairs({
  'fff.conf',
  'fff.file_picker.preview',
  'fff.file_picker.icons',
  'fff.utils',
  'fff.highlights',
}) do
  assert_module(name)
end

local plus = assert_module('fff_plus')
plus.setup()

assert(type(plus.buffers) == 'function', 'fff_plus.buffers should load with real upstream')
assert(type(plus.colors) == 'function', 'fff_plus.colors should load with real upstream')
assert(type(plus.git_status) == 'function', 'fff_plus.git_status should load with real upstream')

for _, name in ipairs({
  'fff_plus.pickers.buffers',
  'fff_plus.pickers.colors',
  'fff_plus.pickers.git_files',
}) do
  assert_module(name)
end

print('Real upstream integration smoke test passed')
