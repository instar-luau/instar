-- instar-lint ignore divide_by_zero
local statement = 1 / 0
-- instar-lint off divide_by_zero
local region = 2 / 0
-- instar-lint on
local visible = 3 / 0
-- instar-lint ignore nonexistent
local unknown = 4 / 0
-- instar-lint ignore divide_by_zero
local ineffective = 5 / 1
return statement, region, visible, unknown, ineffective
