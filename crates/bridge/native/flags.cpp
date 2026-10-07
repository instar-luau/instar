#include "flags.hpp"

#include "Luau/Common.h"
#include "instar-bridge/src/boundary.rs.h"

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
            }
        }
    }

    rust::Vec<NativeFlag> normalize_flags(rust::Slice<const NativeFlag> flags) {
        validate_flags(flags);
        rust::Vec<NativeFlag> result;

        for (const auto &flag : flags) {
            const std::string name(flag.name);

            if (flag.is_boolean) {
                const auto *registered = registered_flag<bool>(name);

                if (registered->value != flag.boolean) {
                    result.push_back(NativeFlag{rust::String(registered->name), flag.boolean, 0, true});
                }
            } else {
                const auto *registered = registered_flag<int>(name);

                if (registered->value != flag.integer) {
                    result.push_back(NativeFlag{rust::String(registered->name), false, flag.integer, false});
                }
            }
        }

        return result;
    }

    void apply_flags(rust::Slice<const NativeFlag> flags) {
        validate_flags(flags);

        for (const auto &flag : flags) {
            const std::string name(flag.name);

            if (flag.is_boolean) {
                registered_flag<bool>(name)->value = flag.boolean;
            } else {
                registered_flag<int>(name)->value = flag.integer;
            }
        }
    }
}
