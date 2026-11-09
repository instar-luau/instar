#include "exports.hpp"

#include "Luau/TypePack.h"
#include "host.hpp"
#include "types.hpp"

#include <map>
#include <optional>
#include <set>
#include <unordered_set>
#include <utility>

namespace instar::luau {
    namespace {
        using Members = std::map<std::string, bool>;
        using Types = std::map<std::string, std::pair<std::string, std::string>>;

        void intersect(Members &left, const Members &right) {
            for (auto iterator = left.begin(); iterator != left.end();) {
                const auto found = right.find(iterator->first);

                if (found == right.end()) {
                    iterator = left.erase(iterator);
                } else {
                    iterator->second = iterator->second && found->second;
                    ++iterator;
                }
            }
        }

        Members members(Luau::TypeId type, PresentationBudget &budget, std::unordered_set<Luau::TypeId> &active) {
            budget.check();
            type = Luau::follow(type);

            if (!active.insert(type).second) {
                return {};
            }

            Members result;

            if (const auto *table = Luau::get<Luau::TableType>(type)) {
                for (const auto &[name, property] : table->props) {
                    budget.check();

                    if (property.readTy) {
                        bool has_receiver = true;
                        result.emplace(name, is_callable(*property.readTy, budget, has_receiver));
                    }
                }
            } else if (const auto *external = Luau::get<Luau::ExternType>(type)) {
                if (external->parent) {
                    result = members(*external->parent, budget, active);
                }

                for (const auto &[name, property] : external->props) {
                    budget.check();

                    if (property.readTy) {
                        bool has_receiver = true;
                        result.insert_or_assign(name, is_callable(*property.readTy, budget, has_receiver));
                    }
                }
            } else if (const auto *metatable = Luau::get<Luau::MetatableType>(type)) {
                result = members(metatable->table, budget, active);
                const auto *table = Luau::get<Luau::TableType>(Luau::follow(metatable->metatable));

                if (table) {
                    const auto found = table->props.find("__index");

                    if (found != table->props.end()) {
                        if (const auto index = found->second.readTy) {
                            const auto inherited = members(*index, budget, active);
                            result.insert(inherited.begin(), inherited.end());
                        }
                    }
                }
            } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(type)) {
                for (const auto part : intersection->parts) {
                    for (const auto &[name, is_function] : members(part, budget, active)) {
                        const auto [iterator, inserted] = result.emplace(name, is_function);

                        if (!inserted) {
                            iterator->second = iterator->second && is_function;
                        }
                    }
                }
            } else if (const auto *union_type = Luau::get<Luau::UnionType>(type)) {
                std::optional<Members> common;

                for (const auto option : union_type->options) {
                    auto candidate = members(option, budget, active);

                    if (common) {
                        intersect(*common, candidate);
                    } else {
                        common = std::move(candidate);
                    }
                }

                result = common.value_or(Members{});
            }

            active.erase(type);

            return result;
        }

        Types types(const Luau::Module &module, PresentationBudget &budget) {
            Types result;

            for (const auto &[name, binding] : module.exportedTypeBindings) {
                budget.check();
                Printer printer(budget, nullptr, true);
                std::string parameters;
                std::string arguments;

                const auto append = [&](const std::string &parameter, const std::string &default_value) {
                    if (!parameters.empty()) {
                        parameters += ", ";
                        arguments += ", ";
                    }

                    parameters += parameter;
                    arguments += parameter;

                    if (!default_value.empty()) {
                        parameters += " = " + default_value;
                    }
                };

                for (const auto &parameter : binding.typeParams) {
                    const auto parameter_name = printer.type(parameter.ty);
                    append(parameter_name, parameter.defaultValue ? printer.type(*parameter.defaultValue, true) : "");
                }

                for (const auto &parameter : binding.typePackParams) {
                    const auto parameter_name = printer.pack(parameter.tp);
                    std::string default_value;

                    if (parameter.defaultValue) {
                        const auto value = Luau::follow(*parameter.defaultValue);

                        if (const auto *pack = Luau::get<Luau::TypePack>(value);
                            pack && pack->head.empty() && !pack->tail) {
                            default_value = "()";
                        } else {
                            default_value = printer.pack(value, true);

                            if (Luau::get<Luau::TypePack>(value)) {
                                default_value = "(" + default_value + ")";
                            }
                        }
                    }

                    append(parameter_name, default_value);
                }

                if (printer.is_complete) {
                    if (!parameters.empty()) {
                        parameters = "<" + parameters + ">";
                        arguments = "<" + arguments + ">";
                    }

                    result.emplace(name, std::pair{std::move(parameters), std::move(arguments)});
                }
            }

            return result;
        }
    }

    Exports exports(
        Luau::Frontend &frontend, const Host &host, const std::vector<std::string> &names, PresentationBudget &budget
    ) {
        Exports result;
        std::optional<Members> common;
        std::optional<Types> aliases;

        for (const auto &name : names) {
            budget.check();
            const auto module = frontend.moduleResolver.getModule(name);
            Members candidate;

            if (module && module->returnType) {
                if (const auto returned = Luau::first(module->returnType)) {
                    std::unordered_set<Luau::TypeId> active;
                    candidate = members(*returned, budget, active);
                }
            }

            if (common) {
                intersect(*common, candidate);
            } else {
                common = std::move(candidate);
            }

            auto candidate_types = module ? types(*module, budget) : Types{};

            if (aliases) {
                for (auto iterator = aliases->begin(); iterator != aliases->end();) {
                    const auto found = candidate_types.find(iterator->first);

                    if (found == candidate_types.end() || found->second != iterator->second) {
                        iterator = aliases->erase(iterator);
                    } else {
                        ++iterator;
                    }
                }
            } else {
                aliases = std::move(candidate_types);
            }
        }

        if (common) {
            for (const auto &[name, is_function] : *common) {
                result.members.push_back({name, is_function});
            }
        }

        if (aliases) {
            for (const auto &[name, signature] : *aliases) {
                result.types.push_back({name, signature.first, signature.second});
            }
        }

        auto pending = names;
        std::unordered_set<std::string> visited;
        std::set<std::string> dependencies;

        while (!pending.empty()) {
            budget.check();
            auto name = std::move(pending.back());
            pending.pop_back();

            if (!visited.insert(name).second) {
                continue;
            }

            dependencies.insert(host.getHumanReadableModuleName(name));
            const auto found = frontend.sourceNodes.find(name);

            if (found != frontend.sourceNodes.end()) {
                pending.insert(pending.end(), found->second->requireSet.begin(), found->second->requireSet.end());
            }
        }

        for (const auto &dependency : dependencies) {
            result.dependencies.push_back(dependency);
        }

        return result;
    }
}
