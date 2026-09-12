local __instar_modules: { [string]: () -> unknown } = {}
local __instar_results: { [string]: { value: unknown } } = {}
local __instar_loading: { [string]: thread | boolean } = {}

local function __instar_require(name: string)
    local cached = __instar_results[name]

    if cached ~= nil then
        return cached.value
    end

    local current: thread | boolean = coroutine.running() or true
    local owner = __instar_loading[name]

    if owner ~= nil then
        if owner == current then
            error("cyclic initialization of " .. name, 2)
        end

        error("concurrent initialization of " .. name .. " is unsupported in a bundle", 2)
    end

    local initialize = __instar_modules[name]

    if initialize == nil then
        error("missing bundled module " .. name, 2)
    end

    __instar_loading[name] = current

    local outcome = table.pack(pcall(initialize))

    __instar_loading[name] = nil

    if outcome[1] == false then
        error(outcome[2], 0)
    end

    if outcome.n > 2 or (outcome.n ~= 2 and name ~= __instar_entry) then
        error("bundled modules must return one value: " .. name, 2)
    end

    __instar_results[name] = { value = outcome[2] }

    return outcome[2]
end
