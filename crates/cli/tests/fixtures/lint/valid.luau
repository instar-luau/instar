local module = require('./module')
local required: (number, number) -> number = module.required
local optional: (number, number?) -> number = module.optional
return required(1, 2), optional(1), optional(1, nil), module:method(1), module.variadic(1, 2, 3)
