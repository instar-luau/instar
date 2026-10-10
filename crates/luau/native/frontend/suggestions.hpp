#ifndef INSTAR_FRONTEND_SUGGESTIONS_HEADER
#define INSTAR_FRONTEND_SUGGESTIONS_HEADER

#include "rust/cxx.h"

#include <cstdint>

namespace instar {
    struct Host;
    struct NativeSuggestions;
}

namespace instar::frontend {
    struct State;

    NativeSuggestions suggest(State &state, const Host &host, rust::Str module, uint32_t line, uint32_t column);
}

#endif
