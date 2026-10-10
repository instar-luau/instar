#ifndef INSTAR_LINTING_HEADER
#define INSTAR_LINTING_HEADER

#include "instar-luau/src/boundary/mod.rs.h"

namespace instar::native {
    struct AnalysisState;
    NativeLintResult lint(AnalysisState &state, const NativeLintInput &input, const NativeOperation &operation);
}

#endif
