#include "flags.hpp"

#include "Luau/Common.h"
#include "instar-luau/src/boundary/mod.rs.h"

#include <map>
#include <stdexcept>
#include <string>
#include <unordered_set>

namespace instar {
    namespace {
        template <typename Value> Luau::FValue<Value> *registered_flag(const std::string &name) {
            for (auto *flag = Luau::FValue<Value>::list; flag; flag = flag->next) {
                if (name == flag->name ||
                    (flag->version != 0 && name == std::string(flag->name) + std::to_string(flag->version))) {
                    return flag;
                }
            }

            return nullptr;
        }

        void validate_flags(rust::Slice<const NativeFlag> flags) {
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

            for (const auto &flag : flags) {
                const std::string name(flag.name);

                const void *found = flag.is_boolean ? static_cast<const void *>(registered_flag<bool>(name))
                                                    : static_cast<const void *>(registered_flag<int>(name));

                if (!found) {
                    throw std::invalid_argument("unknown Luau flag or incorrect value type: " + name);
                }

                if (!registered.insert(found).second) {
                    throw std::invalid_argument("duplicate Luau flag alias: " + name);
                }

                if (flag.is_boolean && flag.boolean &&
                    stdout_flags.find(registered_flag<bool>(name)->name) != stdout_flags.end()) {
                    throw std::invalid_argument("Luau flag writes to stdout and is unavailable: " + name);
                }
            }
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
        validate_flags(flags);
        static_cast<void>(registry_defaults());

        for (const auto &flag : flags) {
            const std::string name(flag.name);

            if (flag.is_boolean) {
                registered_flag<bool>(name)->value = flag.boolean;
            } else {
                registered_flag<int>(name)->value = flag.integer;
            }
        }
    }

    rust::Vec<NativeFlag> normalize_flags(rust::Slice<const NativeFlag> flags) {
        validate_flags(flags);
        auto effective = registry_defaults();

        for (const auto &flag : flags) {
            const std::string name(flag.name);

            if (flag.is_boolean) {
                const auto *registered = registered_flag<bool>(name);

                effective.insert_or_assign(
                    registered->name,
                    NativeFlag{rust::String(registered->name), flag.boolean, 0, true}
                );
            } else {
                const auto *registered = registered_flag<int>(name);

                effective.insert_or_assign(
                    registered->name,
                    NativeFlag{rust::String(registered->name), false, flag.integer, false}
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
