#ifndef INSTAR_FRONTEND_DIAGNOSTICS_HEADER
#define INSTAR_FRONTEND_DIAGNOSTICS_HEADER

#include "Luau/Error.h"
#include "instar-bridge/src/boundary.rs.h"

#include <string>

namespace instar::frontend {
    size_t offset(const Host &host, const std::string &name, Luau::Position position);
    NativeKind kind(const Luau::TypeError &error);
    NativeLocation location(const Host &host, const Luau::ModuleName &name, Luau::Location range);
    NativeDiagnostic diagnostic(const Host &host, const Luau::TypeError &error, const std::string &fallback);
}

#endif
