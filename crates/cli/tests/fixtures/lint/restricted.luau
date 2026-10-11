local dynamic = require(path)
local blocked = require('./dependency')
local allowed = require('./module')
return dynamic, blocked, allowed
