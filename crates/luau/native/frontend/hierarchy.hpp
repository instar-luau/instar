#ifndef INSTAR_FRONTEND_HIERARCHY_HEADER
#define INSTAR_FRONTEND_HIERARCHY_HEADER

#include "rust/cxx.h"

#include <cstdint>

namespace instar {
    struct Host;
    struct NativeHierarchy;
}

namespace instar::frontend {
    struct State;

    NativeHierarchy hierarchy(
        State &state, const Host &host, rust::Slice<const rust::String> modules, rust::Str module,
        rust::Slice<const uint32_t> position
    );
}

#endif
