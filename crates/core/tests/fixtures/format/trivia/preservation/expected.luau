dispatch(
	--[[keep]]
)
dispatch(1 --[[argument]], 2)
local values = {
	1 --[[field]],
	2
}
dispatch() --[[tail]]
dispatch() --[[statement]] ;
dispatch()
dispatch(
	-- empty
)
dispatch(
	1, -- argument
	2
)
local lines = {
	1, -- field
	2
}
dispatch() -- tail
dispatch() -- statement
;
dispatch()
do
	dispatch() --[[nested]] ;
	dispatch()
end
