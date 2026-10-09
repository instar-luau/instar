local provider = require("./modules/renamed")
local consumer = require("./clients/use")
return provider.value, consumer
