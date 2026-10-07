#ifndef INSTAR_FLAGS_HEADER
#define INSTAR_FLAGS_HEADER

#include "rust/cxx.h"

namespace instar {
    struct NativeFlag;

    void validate_flags(rust::Slice<const NativeFlag> flags);
    void apply_flags(rust::Slice<const NativeFlag> flags);
    rust::Vec<NativeFlag> normalize_flags(rust::Slice<const NativeFlag> flags);
}

#endif
