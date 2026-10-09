declare extern type Base with
    value: number
end

declare extern type Child extends Base with
    child: string
end

declare extern type Grandchild extends Child with
    grandchild: boolean
end

declare subject: Child
