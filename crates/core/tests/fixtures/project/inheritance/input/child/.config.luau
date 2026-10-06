local names = {}

for index = 1, 2 do
    names[index] = "child" .. index
end

return { luau = { globals = names, aliases = { child = "./modules" }, lint = { LocalUnused = true } } }
