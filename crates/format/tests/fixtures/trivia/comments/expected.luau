--!strict
-- leading
local value = 1 -- retained
if ready then -- branch
	-- nested
	emit(value) -- call
end
-- trailing
