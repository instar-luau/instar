#ifndef INSTAR_LUAU_NATIVE_ENVIRONMENT
#define INSTAR_LUAU_NATIVE_ENVIRONMENT

#include "Luau/Frontend.h"
#include "rust/cxx.h"

#include <exception>
#include <string>

namespace instar::luau {
    struct Diagnostic;
    struct Environment;
    struct Builtin;
    struct BuiltinFlags;

    struct DeclarationFailure : std::exception {
        explicit DeclarationFailure(const rust::Vec<Diagnostic> &diagnostics) noexcept;

        const rust::Vec<Diagnostic> &diagnostics;
    };

    bool builtins(const BuiltinFlags &configuration, rust::Vec<Builtin> &symbols);

    rust::Vec<Diagnostic> prepare(Luau::Frontend &frontend, const std::string &name, const Environment &environment);
}

#endif
