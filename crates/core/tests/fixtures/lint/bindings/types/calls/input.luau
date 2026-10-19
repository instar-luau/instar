type function first(value)
    return value
end
type function second(value)
    return first(value)
end
export type Result = second<number>
