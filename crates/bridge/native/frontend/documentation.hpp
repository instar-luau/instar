#ifndef INSTAR_FRONTEND_DOCUMENTATION_HEADER
#define INSTAR_FRONTEND_DOCUMENTATION_HEADER

#include "Luau/Scope.h"
#include "rust/cxx.h"

#include <string>

namespace instar::frontend {
    struct State;

    rust::String documentation(const State &state, rust::Str module, rust::Str symbol);

    void
    assign_documentation(const Luau::ScopePtr &scope, const std::string &identity, const std::string &namespace_name);
}

#endif
