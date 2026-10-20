#ifndef INSTAR_LUAU_NATIVE_TYPES
#define INSTAR_LUAU_NATIVE_TYPES

#include "Luau/Cancellation.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "instar-bridge/src/bridge.rs.h"

#include <cstddef>
#include <memory>
#include <string>
#include <vector>

namespace Luau {
    struct ModuleResolver;
}

namespace instar::luau {
    struct PresentationLimits {
        explicit PresentationLimits(bool is_expanded);

        std::size_t nodes;
        std::size_t edges;
        std::size_t depth;
        std::size_t bytes;
    };

    struct PresentationBudget {
        explicit PresentationBudget(std::shared_ptr<Luau::FrontendCancellationToken> cancellation);

        std::size_t nodes = 0;
        std::size_t edges = 0;
        std::size_t bytes = 0;
        std::shared_ptr<Luau::FrontendCancellationToken> cancellation;

        void check() const;
    };

    class Extraction {
    public:
        Extraction(
            PresentationBudget &budget, bool is_expanded, Luau::ScopePtr scope, const Luau::Module &module,
            const Luau::SourceModule &source, Luau::BuiltinTypes &builtins, const Luau::ModuleResolver &modules
        );

        ~Extraction();
        std::size_t type(Luau::TypeId value);
        std::size_t pack(Luau::TypePackId value);

        std::size_t named(
            Luau::TypeId value, const std::string &name, const std::string &qualifier,
            const std::vector<Luau::TypeId> &type_arguments, const std::vector<Luau::TypePackId> &pack_arguments
        );

        NativeSnapshot finish(std::size_t root, bool is_root_pack = false);

    private:
        class Traversal;
        std::unique_ptr<Traversal> traversal;
    };
}

#endif
