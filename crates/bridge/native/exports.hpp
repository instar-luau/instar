#ifndef INSTAR_LUAU_NATIVE_EXPORTS
#define INSTAR_LUAU_NATIVE_EXPORTS

#include "Luau/Frontend.h"
#include "instar-bridge/src/bridge.rs.h"

#include <string>
#include <vector>

namespace instar::luau {
    class Host;
    struct PresentationBudget;

    Exports exports(
        Luau::Frontend &frontend, const Host &host, const std::vector<std::string> &names, PresentationBudget &budget
    );
}

#endif
