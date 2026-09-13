#pragma once

#include "Luau/Frontend.h"
#include <functional>
#include <optional>
#include <string>
#include <vector>

using RobloxMetadata = std::function<std::optional<std::string>(const std::string &, size_t)>;

std::vector<Luau::TypeId> prepareRoblox(Luau::Frontend &frontend, const RobloxMetadata &metadata);
