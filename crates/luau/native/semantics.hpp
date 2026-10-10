#ifndef INSTAR_SEMANTICS_HEADER
#define INSTAR_SEMANTICS_HEADER

#include "instar-luau/src/boundary/mod.rs.h"

namespace instar::native {
    struct AnalysisState;
    NativeFactsResult facts(AnalysisState &state, const NativeFactsInput &input, const NativeOperation &operation);

    NativeInspectionResult
    inspect(AnalysisState &state, const NativePositionInput &input, const NativeOperation &operation);
}

#endif
