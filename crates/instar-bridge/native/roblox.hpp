#ifndef ROBLOX_HPP
#define ROBLOX_HPP

#ifdef __cplusplus
#include "Luau/GlobalTypes.h"
#include <string>
#include <unordered_set>
#endif
#include "bridge.hpp"

#ifdef __cplusplus
extern "C" {
#endif
    /**Roblox class metadata.*/
    typedef struct RobloxClass {
        /**Class name.*/
        Text name;
        /**Whether the class is a service.*/
        uint8_t service;
        /**Whether instances can be created.*/
        uint8_t creatable;
    } RobloxClass;

    /**Roblox instance tree node metadata.*/
    typedef struct RobloxNode {
        /**Instance name.*/
        Text name;
        /**Instance class name.*/
        Text class_name;
        /**Parent node index, or `SIZE_MAX` for a root.*/
        size_t parent;
        /**Whether this node corresponds to a source module.*/
        uint8_t has_module;
        /**Source module whose script global refers to this node.*/
        Text module;
    } RobloxNode;

    /**Registers Roblox class magic after loading definitions.*/
    int32_t checker_register_roblox_classes(void *checker, const RobloxClass *classes, size_t class_count, String *error);
    /**Registers a hierarchy before checking sources; changes require a new checker.*/
    int32_t checker_register_roblox_tree(void *checker, const RobloxNode *nodes, size_t node_count, String *error);
#ifdef __cplusplus
}

namespace Luau {
    struct Frontend;
}

namespace instar {
    /**Registers Roblox metadata in Luau's global type environment.*/
    void register_roblox_magic(Luau::GlobalTypes &globals, const RobloxClass *classes, size_t class_count);
    /**Creates node-specific types and module environments for a Roblox hierarchy.*/
    std::unordered_set<std::string> register_roblox_tree(Luau::Frontend &frontend, const RobloxNode *nodes, size_t node_count);
} // namespace instar
#endif

#endif
