local dependency = require("./dependency")
local value = dependency.value
do
    local dependency = { value = 2 }
    print(dependency.value)
end
dependency.value = value
local typed: dependency.Record = { value = 3 }
return dependency["value"]
