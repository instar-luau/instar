#include "analysis.hpp"
#include "completion.hpp"
#include "engine.hpp"
#include "linting.hpp"
#include "semantics.hpp"

#include "instar-luau/src/boundary/mod.rs.h"

#include <limits>
#include <new>
#include <set>
#include <stdexcept>
#include <string>
#include <vector>

namespace instar {
    struct Engine::State final : native::AnalysisState {};

    Engine::Engine() : state(std::make_unique<State>()) {}

    Engine::~Engine() = default;

    NativeAnalysisResult Engine::analyze(const NativeAnalysisRequest &input, const NativeOperation &operation) {
        if (input.modules.empty() || input.roots.empty() || input.purpose > 1) {
            throw std::invalid_argument("invalid captured analysis request");
        }

        std::set<std::string> names;
        bool rebuild = state->purpose != input.purpose;

        for (const auto &module : input.modules) {
            if (module.name.empty() || module.mode > 1 || module.retained.size() > 11 ||
                (module.inference && !module.has_environment) || (module.reporting && !module.inference)) {
                throw std::invalid_argument("invalid captured module requirements");
            }

            std::set<uint8_t> retained;

            for (const auto category : module.retained) {
                if (category > 10 || !module.inference || !retained.insert(category).second) {
                    throw std::invalid_argument("invalid retained semantic category");
                }
            }

            const auto document = operation.read_document(module.name);

            if (!document.found || document.revision != module.revision || document.text != module.text) {
                throw std::invalid_argument("captured source agreement failed");
            }

            const std::string name(module.name);

            if (!names.insert(name).second) {
                throw std::invalid_argument("duplicate captured module identity");
            }

            const auto previous = state->resolver.captured.find(name);

            rebuild = rebuild || (previous != state->resolver.captured.end() &&
                                     previous->second.environment_signature != module.environment_signature);
        }

        std::set<std::string> roots;

        for (const auto &name : input.roots) {
            if (!names.count(std::string(name)) || !roots.insert(std::string(name)).second) {
                throw std::invalid_argument("analysis root is missing or duplicated");
            }
        }

        for (const auto &module : input.modules) {
            std::set<std::pair<size_t, size_t>> sites;

            for (const auto &site : module.sites) {
                if (!site.target.empty() && !names.count(std::string(site.target))) {
                    throw std::invalid_argument("captured static import target is missing");
                }

                if (site.call_start >= site.call_end || site.call_end > module.text.size() ||
                    site.argument_start < site.call_start || site.argument_start > site.argument_end ||
                    site.argument_end > site.call_end || site.request_kind > 2 ||
                    (site.request_kind == 0 && !site.static_resolution) ||
                    (site.request_kind == 2 && site.static_resolution) ||
                    (!site.static_resolution && !site.target.empty()) ||
                    !sites.emplace(site.call_start, site.call_end).second) {
                    throw std::invalid_argument("invalid captured import site");
                }
            }
        }

        const bool changed = !state->reusable || state->captured.signature != input.signature;

        if (changed) {
            if (state->revision == std::numeric_limits<uint64_t>::max()) {
                throw std::runtime_error("frontend revision exhausted");
            }

            const auto revision = state->revision + 1;

            if (rebuild) {
                state = std::make_unique<State>();
            }

            state->revision = revision;
            std::vector<Luau::ModuleName> removed;

            for (const auto &[name, module] : state->resolver.captured) {
                if (!names.count(name)) {
                    removed.push_back(name);
                }
            }

            state->frontend.clearModules(removed);

            for (const auto &name : removed) {
                state->resolver.captured.erase(name);
                state->resolver.configurations.erase(name);
                state->resolver.environments.erase(name);
                state->outcomes.erase(name);
            }
        }

        if (!state->reusable) {
            for (const auto &module : input.modules) {
                state->frontend.markDirty(std::string(module.name));
            }
        }

        try {
            auto result = native::analyze(*state, input, operation);
            state->reusable = result.completion == NativeCompletion::Complete;

            return result;
        } catch (const std::bad_alloc &) {
            const auto revision = state->revision;
            state = std::make_unique<State>();
            state->revision = revision;
            NativeAnalysisResult result{revision, {}, NativeCompletion::Resource};

            for (const auto &module : input.modules) {
                NativeModuleAnalysis observed{};
                observed.name = module.name;
                observed.logical_revision = module.logical_revision;
                observed.completion = NativeCompletion::Resource;
                result.modules.push_back(std::move(observed));
            }

            return result;
        } catch (...) {
            const auto revision = state->revision;
            state = std::make_unique<State>();
            state->revision = revision;
            throw;
        }
    }

    NativeFactsResult Engine::facts(const NativeFactsInput &input, const NativeOperation &operation) {
        native::validate(
            *state,
            input.frontend_revision,
            std::string(input.name),
            input.logical_revision,
            false,
            input.purpose
        );

        try {
            return native::facts(*state, input, operation);
        } catch (const std::bad_alloc &) {
            NativeFactsResult result{};
            result.completion = NativeCompletion::Resource;

            return result;
        }
    }

    NativeLintResult Engine::lint(const NativeLintInput &input, const NativeOperation &operation) {
        native::validate(
            *state,
            input.frontend_revision,
            std::string(input.name),
            input.logical_revision,
            false,
            input.purpose
        );

        try {
            return native::lint(*state, input, operation);
        } catch (const std::bad_alloc &) {
            NativeLintResult result{{}, {}, {}, {}, NativeCompletion::Resource};

            for (const auto &warning : input.warnings) {
                result.availability.push_back(NativeWarningAvailability{warning, 2});
            }

            return result;
        }
    }

    NativeInspectionResult Engine::inspect(const NativePositionInput &input, const NativeOperation &operation) {
        native::validate(
            *state,
            input.frontend_revision,
            std::string(input.name),
            input.logical_revision,
            true,
            input.purpose
        );

        try {
            return native::inspect(*state, input, operation);
        } catch (const std::bad_alloc &) {
            NativeInspectionResult result{};
            result.completion = NativeCompletion::Resource;

            return result;
        }
    }

    NativeCompletions Engine::complete(const NativePositionInput &input, const NativeOperation &operation) {
        native::validate(
            *state,
            input.frontend_revision,
            std::string(input.name),
            input.logical_revision,
            true,
            input.purpose
        );

        if (input.purpose != 1) {
            throw std::invalid_argument("completion requires established editor analysis");
        }

        try {
            return native::complete(*state, input, operation);
        } catch (const std::bad_alloc &) {
            NativeCompletions result{};
            result.completion = NativeCompletion::Resource;

            return result;
        }
    }

    std::unique_ptr<Engine> create_engine() {
        return std::make_unique<Engine>();
    }
}
