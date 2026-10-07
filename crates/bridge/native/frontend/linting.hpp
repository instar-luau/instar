#ifndef INSTAR_FRONTEND_LINTING_HEADER
#define INSTAR_FRONTEND_LINTING_HEADER

#include "instar-bridge/src/boundary.rs.h"

namespace Luau {
    struct SourceModule;
    struct Module;
}

namespace instar::frontend {
    struct State;
    void collect(State &state, const Host &host, const Luau::SourceModule &source, const Luau::Module &module);

    NativeLintResult lint(
        State &state, const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation,
        rust::Slice<const rust::String> semantic_modules
    );
}

#endif
