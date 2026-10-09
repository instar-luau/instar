local dependency = require("./dependency")
local function consumer()
    return dependency.target()
end
return consumer
