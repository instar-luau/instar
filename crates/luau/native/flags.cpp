#include "flags.hpp"

#include "Luau/Common.h"
#include "instar-luau/src/boundary/mod.rs.h"

#include <map>
#include <stdexcept>
#include <string>
#include <unordered_set>
#include <vector>

namespace instar {
    namespace {
        struct ResolvedFlag {
            const NativeFlag &value;
            Luau::FValue<bool> *boolean;
            Luau::FValue<int> *integer;
        };

        template <typename Value> Luau::FValue<Value> *registered_flag(const std::string &name) {
            for (auto *flag = Luau::FValue<Value>::list; flag; flag = flag->next) {
                if (name == flag->name ||
                    (flag->version != 0 && name == std::string(flag->name) + std::to_string(flag->version))) {
                    return flag;
                }
            }

            return nullptr;
        }

        std::vector<ResolvedFlag> validate_flags(rust::Slice<const NativeFlag> flags) {
            static const std::unordered_set<std::string> stdout_flags{
                "DebugLuauLogSolver",
                "DebugLuauLogBindings",
                "DebugLuauLogSolverToJson",
                "DebugLuauLogCFG",
                "DebugLuauDumpCFGJson",
                "DebugLuauLogTypeFamilies",
                "DebugLuauMagicTypes",
            };

            std::unordered_set<const void *> registered;
            std::vector<ResolvedFlag> resolved;
            resolved.reserve(flags.size());

            for (const auto &flag : flags) {
                const std::string name(flag.name);

                auto *boolean = flag.is_boolean ? registered_flag<bool>(name) : nullptr;
                auto *integer = flag.is_boolean ? nullptr : registered_flag<int>(name);

                const void *found =
                    flag.is_boolean ? static_cast<const void *>(boolean) : static_cast<const void *>(integer);

                if (!found) {
                    throw std::invalid_argument("unknown Luau flag or incorrect value type: " + name);
                }

                if (!registered.insert(found).second) {
                    throw std::invalid_argument("duplicate Luau flag alias: " + name);
                }

                if (boolean && flag.boolean && stdout_flags.find(boolean->name) != stdout_flags.end()) {
                    throw std::invalid_argument("Luau flag writes to stdout and is unavailable: " + name);
                }

                resolved.push_back({flag, boolean, integer});
            }

            return resolved;
        }

        const std::map<std::string, NativeFlag> &registry_defaults() {
            static const auto defaults = [] {
                std::map<std::string, NativeFlag> values;
                for (auto *flag = Luau::FValue<bool>::list; flag; flag = flag->next) {
                    values.emplace(flag->name, NativeFlag{rust::String(flag->name), flag->value, 0, true});
                }
                for (auto *flag = Luau::FValue<int>::list; flag; flag = flag->next) {
                    values.emplace(flag->name, NativeFlag{rust::String(flag->name), false, flag->value, false});
                }
                return values;
            }();

            return defaults;
        }
    }

    void apply_flags(rust::Slice<const NativeFlag> flags) {
        const auto resolved = validate_flags(flags);
        static_cast<void>(registry_defaults());

        for (const auto &flag : resolved) {
            if (flag.boolean) {
                flag.boolean->value = flag.value.boolean;
            } else {
                flag.integer->value = flag.value.integer;
            }
        }
    }

    rust::Vec<NativeFlag> normalize_flags(rust::Slice<const NativeFlag> flags) {
        const auto resolved = validate_flags(flags);
        auto effective = registry_defaults();

        for (const auto &flag : resolved) {
            if (flag.boolean) {
                effective.insert_or_assign(
                    flag.boolean->name,
                    NativeFlag{rust::String(flag.boolean->name), flag.value.boolean, 0, true}
                );
            } else {
                effective.insert_or_assign(
                    flag.integer->name,
                    NativeFlag{rust::String(flag.integer->name), false, flag.value.integer, false}
                );
            }
        }

        rust::Vec<NativeFlag> result;

        for (auto &[name, flag] : effective) {
            result.push_back(std::move(flag));
        }

        return result;
    }
}
