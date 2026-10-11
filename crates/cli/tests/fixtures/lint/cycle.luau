local partner = require('./partner')
local module = {}
function module.required(first: number, second: number): number
    return first + second
end
function module.run(): number
    return partner.required(1)
end
return module
