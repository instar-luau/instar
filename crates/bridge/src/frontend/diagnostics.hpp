#ifndef INSTAR_FRONTEND_DIAGNOSTICS_HEADER
#define INSTAR_FRONTEND_DIAGNOSTICS_HEADER

#include "Luau/Error.h"
#include "instar-bridge/src/boundary.rs.h"

namespace instar::frontend {
    size_t offset(const std::string &source, Luau::Position position);
    NativeKind kind(const Luau::TypeError &error);
    NativeLocation location(const Host &host, const Luau::ModuleName &name, Luau::Location range);
    NativeDiagnostic diagnostic(const Host &host, const Luau::TypeError &error, const std::string &fallback);
} // namespace instar::frontend

#endif
