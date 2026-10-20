#ifndef INSTAR_LUAU_NATIVE_NAMING
#define INSTAR_LUAU_NATIVE_NAMING

#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/Type.h"
#include "Luau/TypeArena.h"

#include <cstddef>
#include <string>
#include <vector>

namespace Luau {
    struct ModuleResolver;
}

namespace instar::luau {
    struct PresentationBudget;
    struct PresentationLimits;

    struct Visibility {
        bool is_verified = false;
        std::string qualifier;
    };

    struct TypeName {
        bool is_verified = false;
        std::string name;
        std::string qualifier;
        std::vector<Luau::TypeId> type_arguments;
        std::vector<Luau::TypePackId> pack_arguments;
    };

    class Naming {
    public:
        Naming(
            PresentationBudget &budget, Luau::ScopePtr scope, const Luau::Module &module,
            const Luau::SourceModule &source, const Luau::ModuleResolver &modules, const PresentationLimits &limits
        );

        Visibility table(const Luau::TableType &table);
        TypeName metatable(const Luau::MetatableType &value, Luau::TypeId type, bool expand_structure);

    private:
        PresentationBudget &budget;
        Luau::TypeArena arena;
        Luau::ScopePtr scope;
        const Luau::Module &module;
        const Luau::SourceModule &source;
        const Luau::ModuleResolver &modules;
        const PresentationLimits &limits;
        std::size_t visits = 0;
    };

    bool has_ambiguous_name(Luau::TypeId value, const std::vector<Luau::TypeId> &alternatives);
}

#endif
