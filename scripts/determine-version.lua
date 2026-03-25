local script_path = arg[0]
local script_dir = script_path:match('(.*[/\\])') or './'
local repo_root = script_dir .. '..'

-- Add the plugin's lua/ directory to the module search path
package.path = repo_root .. '/lua/?.lua;' .. repo_root .. '/lua/?/init.lua;' .. package.path

local version = require('fff.utils.version')

local info, err = version.resolve(repo_root)
if not info then
  io.stderr:write('Error: ' .. (err or 'unknown') .. '\n')
  os.exit(1)
end

print('version=' .. info.version)
print('npm_tag=' .. info.npm_tag)
print('is_release=' .. tostring(info.is_release))

-- Write to GITHUB_OUTPUT when running in CI
local github_output = os.getenv('GITHUB_OUTPUT')
if github_output and github_output ~= '' then
  local f = io.open(github_output, 'a')
  if f then
    f:write('version=' .. info.version .. '\n')
    f:write('npm_tag=' .. info.npm_tag .. '\n')
    f:write('is_release=' .. tostring(info.is_release) .. '\n')
    f:close()
  end
end
