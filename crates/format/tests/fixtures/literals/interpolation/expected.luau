local text = `sum={left + right}; value={object:read()[index]}`
local choice = `ready={if enabled then "yes" else "no"}`
local escaped = `literal \{brace}, tick=\`; value={count}`
