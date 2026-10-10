#ifndef INSTAR_PARSING_HEADER
#define INSTAR_PARSING_HEADER

#include "Luau/Error.h"
#include "instar-luau/src/boundary/mod.rs.h"

#include <cstddef>
#include <string>

namespace instar::native {
    struct AnalysisState;
    void parse_modules(AnalysisState &state, const NativeOperation &operation, rust::Slice<const rust::String> names);
    size_t offset(const NativeOperation &operation, const std::string &name, Luau::Position position);
    NativeKind kind(const Luau::TypeError &error);
    NativeLocation location(const NativeOperation &operation, const Luau::ModuleName &name, Luau::Location range);

    NativeDiagnostic
    diagnostic(const NativeOperation &operation, const Luau::TypeError &error, const std::string &fallback);
}

#endif
