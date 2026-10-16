#ifndef INSTAR_LUAU_NATIVE_INSTANCES
#define INSTAR_LUAU_NATIVE_INSTANCES

#include "Luau/GlobalTypes.h"
#include "Luau/Scope.h"
#include "Luau/TypeArena.h"
#include "rust/cxx.h"

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

namespace instar::luau {
    struct Instance;

    enum class InstanceQuery : std::uint8_t {
        Game,
        Workspace,
        Script,
        Parent,
        Property,
        Child,
        Descendant,
        Service,
        Ancestor,
    };

    class Instances {
    public:
        Instances(
            const std::string &name, const Luau::ScopePtr &scope, rust::Slice<const Instance> instances,
            Luau::NotNull<Luau::BuiltinTypes> builtins
        );

        void bind(Luau::GlobalTypes &globals, const Luau::ScopePtr &scope, std::optional<size_t> script) const;
        std::optional<std::string> global(InstanceQuery query, std::optional<size_t> script) const;

        std::optional<std::string>
        resolve(const std::string &context, InstanceQuery query, const std::string &name) const;

    private:
        Luau::TypeArena arena;
        std::vector<Luau::TypeId> types;
        std::unordered_map<std::string, Luau::TypeId> identities;
    };

    std::string instance_identity(const std::string &hierarchy, size_t script);

    std::optional<Luau::TypeId> lookup_instance(Luau::TypeId type, InstanceQuery query, std::string_view name);
}

#endif
