local function require(value: string): number
    return #value
end
return require("./dependency")
