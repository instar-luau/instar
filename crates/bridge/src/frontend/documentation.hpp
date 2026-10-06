#ifndef INSTAR_FRONTEND_DOCUMENTATION_HEADER
#define INSTAR_FRONTEND_DOCUMENTATION_HEADER

#include "Luau/Scope.h"
#include <string>

namespace instar {
    void
    assign_documentation(const Luau::ScopePtr &scope, const std::string &identity, const std::string &namespace_name);
}

#endif
