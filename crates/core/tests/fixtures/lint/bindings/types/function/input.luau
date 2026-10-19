type function identity(value)
    return value
end
type function unused(value)
    return value
end
export type Result = identity<number>
