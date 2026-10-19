local Types = require("types")
declare extern type Item with
    function consume(self, value: Types.Item): Types.Item
end
