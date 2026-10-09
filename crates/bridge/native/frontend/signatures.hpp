#ifndef INSTAR_FRONTEND_SIGNATURES_HEADER
#define INSTAR_FRONTEND_SIGNATURES_HEADER

#include "rust/cxx.h"

#include <cstdint>

namespace instar {
    struct Host;
    struct NativeSignatures;
}

namespace instar::frontend {
    struct State;

    NativeSignatures signatures(State &state, const Host &host, rust::Str module, uint32_t line, uint32_t column);
}

#endif
