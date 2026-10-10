#ifndef INSTAR_COMPLETION_HEADER
#define INSTAR_COMPLETION_HEADER

#include "instar-luau/src/boundary/mod.rs.h"

namespace instar::native {
    struct AnalysisState;

    NativeCompletions
    complete(AnalysisState &state, const NativePositionInput &input, const NativeOperation &operation);
}

#endif
