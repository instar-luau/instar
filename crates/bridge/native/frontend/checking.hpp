#ifndef INSTAR_FRONTEND_CHECKING_HEADER
#define INSTAR_FRONTEND_CHECKING_HEADER

#include "instar-bridge/src/boundary.rs.h"

namespace instar::frontend {
    struct State;

    NativeCheck analyze(
        State &state, const Host &host, rust::Slice<const rust::String> entries, double timeout_seconds,
        rust::Slice<const rust::String> names, const Cancellation &cancellation, bool typecheck
    );
}

#endif
