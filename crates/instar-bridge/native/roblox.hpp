#ifndef ROBLOX_HPP
#define ROBLOX_HPP

#ifdef __cplusplus
#include "Luau/GlobalTypes.h"
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
    } RobloxNode;

    /**Registers Roblox classes and instance nodes with a checker.*/
    int32_t checker_register_roblox(void *checker, const RobloxClass *classes, size_t class_count, const RobloxNode *nodes, size_t node_count, String *error);
#ifdef __cplusplus
}

namespace instar {
    /**Registers Roblox metadata in Luau's global type environment.*/
    void register_roblox_magic(Luau::GlobalTypes &globals, const RobloxClass *classes, size_t class_count, const RobloxNode *nodes, size_t node_count);
}
#endif

#endif
