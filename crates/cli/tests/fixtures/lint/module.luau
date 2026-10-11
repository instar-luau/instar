local module = {}
function module.required(first: number, second: number): number
    return first + second
end
function module.optional(first: number, second: number?): number
    return first + (second or 0)
end
function module.variadic(first: number, ...: number): number
    return first
end
function module:method(first: number, second: number?): number
    return first + (second or 0)
end
return module
