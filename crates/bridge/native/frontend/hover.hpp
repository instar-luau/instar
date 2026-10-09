#ifndef INSTAR_FRONTEND_HOVER_HEADER
#define INSTAR_FRONTEND_HOVER_HEADER

#include "rust/cxx.h"

#include <cstdint>

namespace instar {
    struct Host;
    struct NativeHover;
}

namespace instar::frontend {
    struct State;

    NativeHover hover(State &state, const Host &host, rust::Str module, uint32_t line, uint32_t column);
}

#endif
