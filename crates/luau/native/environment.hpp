#ifndef INSTAR_ENVIRONMENT_HEADER
#define INSTAR_ENVIRONMENT_HEADER

#include "instar-luau/src/boundary/mod.rs.h"

namespace instar::native {
    struct AnalysisState;

    struct EnvironmentPreparation {
        rust::Vec<NativeDiagnostic> diagnostics;
        NativeCompletion completion;
    };

    EnvironmentPreparation
    prepare_environment(AnalysisState &state, const NativeModuleInput &input, const NativeOperation &operation);
}

#endif
