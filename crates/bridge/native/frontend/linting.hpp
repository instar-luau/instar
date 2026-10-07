#ifndef INSTAR_FRONTEND_LINTING_HEADER
#define INSTAR_FRONTEND_LINTING_HEADER

#include "instar-bridge/src/boundary.rs.h"

namespace instar::frontend {
    struct State;

    NativeLintResult lint(
        State &state, const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation,
        rust::Slice<const rust::String> semantic_modules
    );
}

#endif
