assert(require == nil)
assert(io == nil)
assert(loadfile == nil)
assert(dofile == nil)
assert(socket == nil)
assert(http == nil)
assert(os.execute == nil)
assert(os.remove == nil)

return { luau = { languagemode = "strict" } }
