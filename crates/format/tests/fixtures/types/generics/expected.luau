export type Box<Value = string> = { value: Value }
type Mapper<Input, Output...> = (Input) -> (Output...)
local function apply<Value, Results...>(value: Value, callback: (Value) -> (Results...)): Results...
	return callback(value)
end
local result = factory<<Box<number>?>>(nil)
local cast = (result :: Box<number>)
type Runtime = typeof(result)
