local module = require('./module')
local required: (number, number) -> number = module.required
local optional: (number, number?) -> number = module.optional
local results = {
    required(1),
    required(1, 2, 3),
    optional(),
    module:method(),
    module.optional(1, 2, 3),
}
return results
