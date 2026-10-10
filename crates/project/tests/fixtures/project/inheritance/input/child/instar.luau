local names = {}

for index = 1, 2 do
    names[index] = "child" .. index
end

return { ["formatting"] = { ["indent_style"] = "spaces", ["requires"] = { ["groups"] = { "relative", "other" } } }, ["linting"] = { ["rules"] = { ["restricted_globals"] = { ["enabled"] = true, ["severity"] = "Warning", ["settings"] = { ["rule"] = "restricted_globals", ["parameters"] = { ["names"] = { ["old"] = "child" } } } } }, ["warnings"] = { ["LocalUnused"] = true } }, ["environment"] = { ["groups"] = { { ["namespace"] = "@child", ["documentation"] = { "child.json" } } }, ["globals"] = names }, ["editing"] = { ["hints"] = { ["parameter_names"] = true } }, ["selection"] = { ["exclude"] = {  } }, ["resolution"] = { ["aliases"] = { ["child"] = "./modules" } } }
