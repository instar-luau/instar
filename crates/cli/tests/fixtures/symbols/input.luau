type Record = {value: number}
local table = {
    ["𐐀"] = 1,
    nested = {answer = 42},
}
local function outer(value: Record)
    local function inner()
        return value
    end
    return inner()
end
