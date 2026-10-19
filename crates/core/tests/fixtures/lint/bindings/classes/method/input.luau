local Types = require("types")
class Item
    function consume(self, value: Types.Item)
        return self, value
    end
end
