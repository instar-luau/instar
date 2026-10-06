local root = "./library"
local aliases = {Root = root, inherited = "./parent", selected = "./parent", chain = "@ROOT/leaf", down = "@childonly", first = "@second", second = "@FIRST"}

for _, name in {"one", "two"} do
 aliases[name] = root .. "/leaf"
end

return {luau = {aliases = aliases}}
