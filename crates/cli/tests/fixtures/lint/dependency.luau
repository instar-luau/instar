local unused = 1
return function(first: number, second: number?): number
    return first + (second or 0)
end
