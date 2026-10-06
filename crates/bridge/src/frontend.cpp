#include "instar-bridge/src/lib.rs.h"

#include "Luau/Ast.h"
#include "Luau/ConfigResolver.h"
#include "Luau/Frontend.h"
#include "Luau/Module.h"

#include <map>
#include <stdexcept>
#include <string>
#include <utility>

namespace instar {
    namespace {
        std::string text(rust::Str value) {
            return std::string(value.data(), value.size());
        }

        size_t offset(const std::string &source, Luau::Position position) {
            size_t start = 0;

            for (unsigned int line = 0; line < position.line; ++line) {
                const size_t newline = source.find('\n', start);

                if (newline == std::string::npos) {
                    throw std::runtime_error("native source position exceeds host revision");
                }

                start = newline + 1;
            }

            if (position.column > source.size() - start) {
                throw std::runtime_error("native source column exceeds host revision");
            }

            return start + position.column;
        }

        struct Resolver final : Luau::FileResolver, Luau::ConfigResolver {
            const Host *host = nullptr;
            std::string active;
            std::string activeSource;
            std::map<std::string, Luau::Config> configurations;

            std::optional<Luau::SourceCode> readSource(const Luau::ModuleName &name) override {
                if (!host) {
                    throw std::runtime_error("host callbacks are unavailable");
                }

                const NativeSource source = host->read_source(name);

                if (!source.found) {
                    return std::nullopt;
                }

                active = name;
                activeSource = std::string(source.text);

                return Luau::SourceCode{std::string(source.text), Luau::SourceCode::Module};
            }

            std::optional<Luau::ModuleInfo>
            resolveModule(const Luau::ModuleInfo *, Luau::AstExpr *expression, const Luau::TypeCheckLimits &) override {
                if (!host) {
                    throw std::runtime_error("host callbacks are unavailable");
                }

                const rust::String target = host->resolve(
                    active,
                    offset(activeSource, expression->location.begin),
                    offset(activeSource, expression->location.end)
                );

                if (target.empty()) {
                    return std::nullopt;
                }

                return Luau::ModuleInfo{std::string(target)};
            }

            const Luau::Config &getConfig(const Luau::ModuleName &name, const Luau::TypeCheckLimits &) const override {
                const auto found = configurations.find(name);

                if (found == configurations.end()) {
                    throw std::runtime_error("host configuration is unavailable: " + name);
                }

                return found->second;
            }
        };

        struct Calls final : Luau::AstVisitor {
            const std::string &source;
            std::map<std::pair<size_t, size_t>, Luau::AstExprCall *> calls;

            explicit Calls(const std::string &source) : source(source) {}

            bool visit(Luau::AstExprCall *call) override {
                calls.emplace(
                    std::make_pair(offset(source, call->location.begin), offset(source, call->location.end)),
                    call
                );

                return true;
            }

            bool visit(Luau::AstType *) override { return true; }

            bool visit(Luau::AstTypePack *) override { return true; }
        };
    } // namespace

    struct NativeFrontend::State {
        Resolver resolver;
        Luau::Frontend frontend{&resolver, &resolver};
    };

    NativeFrontend::NativeFrontend() : state(std::make_unique<State>()) {}

    NativeFrontend::~NativeFrontend() = default;

    std::unique_ptr<NativeFrontend> create_frontend() {
        return std::make_unique<NativeFrontend>();
    }

    void NativeFrontend::configure(rust::Str name, const NativeConfiguration &configuration) {
        const std::string module = text(name);
        state->frontend.markDirty(module);
        state->resolver.configurations.insert_or_assign(module, configuration.value());
    }

    rust::Vec<NativeLink> NativeFrontend::prepare(const Host &host, rust::Slice<const rust::String> names) {
        auto &frontend = state->frontend;
        state->resolver.host = &host;

        struct Reset {
            Resolver &resolver;
            ~Reset() { resolver.host = nullptr; }
        } reset{state->resolver};

        std::vector<Luau::ModuleName> modules;

        for (const rust::String &name : names) {
            modules.emplace_back(name);
        }

        frontend.parseModules(modules);

        rust::Vec<NativeLink> links;

        for (const std::string &name : modules) {
            const NativeSource source = host.read_source(name);
            Luau::SourceModule *module = frontend.getSourceModule(name);
            const auto node = frontend.sourceNodes.find(name);

            if (!source.found || !module || !module->root || node == frontend.sourceNodes.end()) {
                throw std::runtime_error("native module source is unavailable: " + name);
            }

            const std::string bytes(source.text);
            Calls calls(bytes);
            module->root->visit(&calls);
            Luau::RequireTraceResult trace;
            node->second->requireSet.clear();
            node->second->requireLocations.clear();

            for (const NativeSite &site : source.sites) {
                const auto found = calls.calls.find({site.call_start, site.call_end});

                if (found == calls.calls.end()) {
                    if (site.static_request) {
                        throw std::runtime_error("host/native static require call disagreement: " + name);
                    }

                    continue;
                }

                Luau::AstExprCall *call = found->second;

                if (site.static_request &&
                    (call->args.size != 1 || offset(bytes, call->args.data[0]->location.begin) != site.argument_start ||
                        offset(bytes, call->args.data[0]->location.end) != site.argument_end)) {
                    throw std::runtime_error("host/native static require argument disagreement: " + name);
                }

                const std::string target(site.target);

                if (target.empty()) {
                    trace.exprs[call] = {};
                    continue;
                }

                if (frontend.sourceNodes.find(target) == frontend.sourceNodes.end()) {
                    throw std::runtime_error("host target was not parsed: " + target);
                }

                trace.exprs[call->args.data[0]] = Luau::ModuleInfo{target};
                trace.exprs[call] = Luau::ModuleInfo{target};
                trace.requireList.emplace_back(target, call->location);
                node->second->requireSet.insert(target);
                node->second->requireLocations.emplace_back(target, call->location);

                links.push_back(
                    NativeLink{rust::String(name),
                        source.revision,
                        site.call_start,
                        site.call_end,
                        site.argument_start,
                        site.argument_end,
                        rust::String(target)}
                );
            }

            frontend.requireTrace.insert_or_assign(name, std::move(trace));
        }

        for (const auto &entry : frontend.sourceNodes) {
            entry.second->dependents.clear();
        }

        for (const auto &[name, node] : frontend.sourceNodes) {
            for (const std::string &target : node->requireSet) {
                if (const auto found = frontend.sourceNodes.find(target); found != frontend.sourceNodes.end()) {
                    found->second->dependents.insert(name);
                }
            }
        }

        return links;
    }

    void NativeFrontend::invalidate(rust::Slice<const rust::String> names) {
        std::vector<Luau::ModuleName> modules;

        for (const rust::String &name : names) {
            modules.emplace_back(name);
            state->resolver.configurations.erase(std::string(name));
        }

        state->frontend.clearModules(modules);
    }
} // namespace instar
