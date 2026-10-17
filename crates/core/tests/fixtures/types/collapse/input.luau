do
type Record={
name:string,
enabled:boolean
}
type List={
number
}
type Empty={ }
end

do
type Record={compact:{value:number},expanded:{value:string,},read [string]:{number},}
end

do
type Record={name:string,enabled:boolean,}
type List={[number]:number,}
end

type function identity(value) return value end
type Result=identity<string>

export type Box<Value=string> ={value:Value}
type Mapper<Input,Output...> =(Input)->(Output...)
local function apply<Value,Results...>(value:Value,callback:(Value)->(Results...)):Results... return callback(value) end
local result=factory<<Box<number>?>>(nil)
local cast=(result::Box<number>)
type Runtime=typeof (result)

type Variant=string|number|nil
type Combined=Base&{read name:"ready",write enabled:boolean,}
local choice=if ready then (value::Variant) else nil
