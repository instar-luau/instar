return table.freeze({
    data = buffer.fromstring((
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .. "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        .. "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
    )),
})
