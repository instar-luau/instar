local text=`sum={left+right}; value={object:read()[index]}`
local choice=`ready={if enabled then "yes" else "no"}`
local escaped=`literal \{brace}, tick=\`; value={count}`

print(`{value}`,0)
print(`{value}` ,0)
print(`{value}` )

local hexadecimal=0xAB_CD
local binary=0b1010_0101
local exponent=1_234.50e+02
local fraction=.5
local mantissa=3.141_592
