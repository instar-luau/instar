local cycle = require('./cycle')
local module = {}
function module.required(first: number, second: number): number
    return first + second
end
function module.run(): number
    return cycle.required(1)
end
return module
