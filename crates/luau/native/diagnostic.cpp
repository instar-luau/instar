#include "diagnostic.hpp"

#include "Luau/Module.h"
#include "instar-luau/src/bridge.rs.h"

namespace instar::luau {
    void attribute(Luau::Module &module) {
        for (auto &error : module.errors) {
            if (error.moduleName.empty()) {
                error.moduleName = module.name;
            }
        }
    }

    Diagnostic diagnose(const Luau::TypeError &error, Luau::FileResolver &resolver) {
        return Diagnostic{
            resolver.getHumanReadableModuleName(error.moduleName),
            error.code(),
            error.location.begin.line,
            error.location.begin.column,
            error.location.end.line,
            error.location.end.column,
            Luau::toString(error, Luau::TypeErrorToStringOptions{&resolver}),
        };
    }
}
