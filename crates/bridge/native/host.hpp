#ifndef INSTAR_LUAU_NATIVE_HOST
#define INSTAR_LUAU_NATIVE_HOST

#include "Luau/ConfigResolver.h"
#include "Luau/FileResolver.h"
#include "flags.hpp"
#include "instances.hpp"

#include <map>
#include <memory>
#include <optional>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

namespace Luau {
    struct Frontend;
}

namespace instar::luau {
    struct Context;
    struct Environment;
    class Frontend;

    class Host final : public Luau::FileResolver, public Luau::ConfigResolver {
    public:
        Host(const Context &context, const Flags &flags, Frontend &frontend);

        std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override;
        std::string getHumanReadableModuleName(const Luau::ModuleName &name) const override;

        std::optional<Luau::ModuleInfo> resolveModule(
            const Luau::ModuleInfo *context, Luau::AstExpr *expression, const Luau::TypeCheckLimits &limits
        ) override;

        const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &limits) const override;

        std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName &name) const override;
        Environment environment(const std::string &name) const;

        const Instances *instances(Luau::Frontend &native, const Luau::ModuleName &name) const;
        void bind(Luau::Frontend &native, const Luau::ModuleName &name, const Luau::ScopePtr &scope) const;

        std::vector<std::string> modules(const Luau::ModuleName &name) const;
        void invalidate(const Luau::ModuleName &name);
        void clear();

    private:
        struct Settings {
            Luau::Config configuration;
            std::optional<std::string> environment;
            std::optional<std::string> hierarchy;
            std::optional<size_t> script;
            std::vector<std::string> modules;
        };

        struct Mapping {
            std::optional<std::string> source;
            std::string hierarchy;
            size_t script;
            Luau::SourceCode::Type type;
        };

        const Settings &resolve(const Luau::ModuleName &name) const;
        const rust::Vec<Instance> &load(const std::string &name) const;

        const Context &callbacks;
        const Flags &flags;
        Frontend &frontend;
        mutable std::unordered_map<Luau::ModuleName, Settings> configurations;
        mutable std::unordered_map<std::string, rust::Vec<Instance>> models;
        mutable std::unordered_map<Luau::ModuleName, Mapping> mappings;
        mutable std::map<std::pair<std::string, std::string>, std::unique_ptr<Instances>> hierarchies;
        const Instances *hierarchy = nullptr;
    };
}

#endif
