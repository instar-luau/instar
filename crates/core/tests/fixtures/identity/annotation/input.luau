do
local zeta = require('./z')
local alpha: typeof(zeta) = require('./a')
end

do
local value = require('./z')
local value = require('./a')
end

do
local require = function(value) return value end
local zeta = require('./z')
local alpha = require('./a')
end

do
local function load(require)
local zeta = require('./z')
local alpha = require('./a')
end
end

do
local zeta = require('./z')
local require = require('./a')
end
