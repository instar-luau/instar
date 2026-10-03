#ifndef ROBLOX_HPP
#define ROBLOX_HPP

#include "Luau/GlobalTypes.h"
#include "bridge.hpp"

#include <string>
#include <unordered_set>

namespace instar {
    void register_roblox_magic(Luau::GlobalTypes &globals, rust::Slice<const RobloxClass> classes);
    std::unordered_set<std::string> register_roblox_tree(Luau::Frontend &frontend, rust::Slice<const RobloxNode> nodes);
} // namespace instar

#endif
