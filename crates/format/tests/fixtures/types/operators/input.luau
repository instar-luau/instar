type Variant=string|number|nil
type Combined=Base&{read name:"ready",write enabled:boolean,}
local choice=if ready then (value::Variant) else nil
