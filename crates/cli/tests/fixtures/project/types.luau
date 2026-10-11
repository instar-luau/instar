declare extern type Base with
    value: number
end

declare extern type Child extends Base with
    label: string
end

declare subject: Child
declare function documented(value: number): string
