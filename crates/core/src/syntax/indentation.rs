pub(crate) fn indentation(bytes: &[u8], width: usize) -> usize {
    bytes.iter().fold(0, |columns, byte| {
        columns
            + if *byte == b'\t' {
                width - columns % width
            } else {
                1
            }
    })
}
