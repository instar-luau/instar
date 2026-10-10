#include "diagnostics.hpp"
#include "parsing.hpp"
#include "state.hpp"

#include "Luau/Ast.h"

#include <map>
#include <stdexcept>
#include <utility>

namespace instar::frontend {
    namespace {
        struct Calls final : Luau::AstVisitor {
            const Host &host;
            const std::string &name;
            std::map<std::pair<size_t, size_t>, Luau::AstExprCall *> calls;

            Calls(const Host &host, const std::string &name) : host(host), name(name) {}

            bool visit(Luau::AstExprCall *call) override {
                calls.emplace(
                    std::make_pair(offset(host, name, call->location.begin), offset(host, name, call->location.end)),
                    call
                );

                return true;
            }

            bool visit(Luau::AstType *) override { return true; }

            bool visit(Luau::AstTypePack *) override { return true; }
        };
    }

    rust::Vec<NativeLink> prepare(State &state, const Host &host, rust::Slice<const rust::String> names) {
        auto &frontend = state.frontend;
        state.resolver.host = &host;

        struct Reset {
            Resolver &resolver;

            ~Reset() { resolver.host = nullptr; }
        } reset{state.resolver};

        std::vector<Luau::ModuleName> modules;

        for (const rust::String &name : names) {
            modules.emplace_back(name);
        }

        host.progress("Preparing native graph", "", 0, modules.size());
        frontend.parseModules(modules);

        rust::Vec<NativeLink> links;
        size_t completed = 0;

        for (const std::string &name : modules) {
            const NativeModule source = host.read_module(name);
            Luau::SourceModule *module = frontend.getSourceModule(name);
            const auto node = frontend.sourceNodes.find(name);

            if (!source.found || !module || !module->root || node == frontend.sourceNodes.end()) {
                throw std::runtime_error("native module source is unavailable: " + name);
            }

            Calls calls(host, name);

            if (!source.sites.empty()) {
                module->root->visit(&calls);
            }

            Luau::RequireTraceResult trace;
            node->second->requireSet.clear();
            node->second->requireLocations.clear();

            for (const NativeSite &site : source.sites) {
                const auto found = calls.calls.find({site.call_start, site.call_end});

                if (found == calls.calls.end()) {
                    if (site.static_request && module->parseErrors.empty()) {
                        throw std::runtime_error("host/native static require call disagreement: " + name);
                    }

                    continue;
                }

                Luau::AstExprCall *call = found->second;

                if (site.static_request &&
                    (call->args.size != 1 ||
                        offset(host, name, call->args.data[0]->location.begin) != site.argument_start ||
                        offset(host, name, call->args.data[0]->location.end) != site.argument_end)) {
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
            host.progress("Preparing native graph", name, ++completed, modules.size());
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
}
