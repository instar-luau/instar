do
local z = require('./z')
local a: typeof(z) = require('./a')
end

do
local value = require('./z')
local value = require('./a')
end

do
local require = function(value) return value end
local z = require('./z')
local a = require('./a')
end

do
local function load(require)
local z = require('./z')
local a = require('./a')
end
end

do
local z = require('./z')
local require = require('./a')
end
