#ifndef INSTAR_FRONTEND_PARSING_HEADER
#define INSTAR_FRONTEND_PARSING_HEADER

#include "instar-bridge/src/boundary.rs.h"

namespace instar::frontend {
    struct State;

    rust::Vec<NativeLink> prepare(State &state, const Host &host, rust::Slice<const rust::String> names);
}

#endif
