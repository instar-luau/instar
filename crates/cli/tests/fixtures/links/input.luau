local banner = "𝑥"; local module = require("./module")
local aliased = require("@modules/module")
local missing = require("./missing")
do
    local require = function(value) return value end
    local shadowed = require("./module")
end
-- 𝑥 [documentation](https://example.com/reference_(value))
-- <HTTPS://example.com/second>
-- https://
-- nothttps://example.com/ignored
-- javascript:alert(1)
local literal = "https://example.com/not-comment"
return module, aliased, missing, banner, literal
