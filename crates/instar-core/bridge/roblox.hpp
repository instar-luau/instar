#pragma once

#include "Luau/GlobalTypes.h"

#include <string>
#include <vector>

namespace instar {

    struct RobloxClass {
        std::string name;
        bool service;
        bool creatable;
    };

    struct RobloxNode {
        std::string name;
        std::string class_name;
    };

    void register_roblox_magic(
        Luau::GlobalTypes &globals, const std::vector<RobloxClass> &classes, const std::vector<RobloxNode> &nodes);

} // namespace instar
