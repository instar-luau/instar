local dependency = require("./dependency")
local value = dependency.amount
do
    local dependency = { value = 2 }
    print(dependency.value)
end
dependency.amount = value
local typed: dependency.Record = { value = 3 }
return dependency["amount"]
