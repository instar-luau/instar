#ifndef INSTAR_LUAU_NATIVE_ENVIRONMENT
#define INSTAR_LUAU_NATIVE_ENVIRONMENT

#include "Luau/Frontend.h"
#include "rust/cxx.h"

#include <exception>
#include <string>

namespace instar::luau {
    struct Context;
    struct Diagnostic;
    struct Environment;
    struct Flag;

    struct DeclarationFailure : std::exception {
        explicit DeclarationFailure(const rust::Vec<Diagnostic> &diagnostics) noexcept;

        const rust::Vec<Diagnostic> &diagnostics;
    };

    rust::String builtins(const Context &host, rust::Slice<const Flag> scoped);
    rust::Vec<Diagnostic> prepare(Luau::Frontend &frontend, const std::string &name, const Environment &environment);
}

#endif
