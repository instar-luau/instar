#include "reduction.hpp"

#include "Luau/Clone.h"
#include "Luau/Normalize.h"
#include "Luau/Subtyping.h"
#include "Luau/TypeFunctionRuntime.h"
#include "Luau/UnifierSharedState.h"
#include "types.hpp"

#include <unordered_set>

namespace instar::luau {
    std::vector<Luau::TypeId> reduce_union(
        const Luau::UnionType &value, PresentationBudget &budget, Luau::BuiltinTypes &builtin_types,
        const PresentationLimits &limits
    ) {
        std::vector<Luau::TypeId> pending(value.options.rbegin(), value.options.rend());
        std::vector<Luau::TypeId> members;
        std::unordered_set<Luau::TypeId> seen;
        std::size_t visits = 0;

        while (!pending.empty()) {
            budget.check();

            if (++visits > limits.edges) {
                return value.options;
            }

            const auto member = Luau::follow(pending.back(), Luau::FollowOption::DisableLazyTypeThunks);
            pending.pop_back();

            if (!seen.insert(member).second) {
                continue;
            }

            if (const auto *nested = Luau::get<Luau::UnionType>(member)) {
                if (nested->options.size() > limits.edges - visits) {
                    return value.options;
                }

                pending.insert(pending.end(), nested->options.rbegin(), nested->options.rend());
            } else {
                members.push_back(member);
            }
        }

        if (members.empty()) {
            return value.options;
        }

        if (members.size() < 2) {
            return members;
        }

        Luau::TypeArena arena;
        Luau::InternalErrorReporter reporter;
        Luau::UnifierSharedState shared(&reporter);
        const Luau::NotNull builtins{&builtin_types};
        Luau::Normalizer normalizer(&arena, builtins, Luau::NotNull{&shared}, Luau::SolverMode::New);
        Luau::TypeCheckLimits checking;
        checking.cancellationToken = budget.cancellation;
        Luau::TypeFunctionRuntime runtime(Luau::NotNull{&reporter}, Luau::NotNull{&checking});
        runtime.allowEvaluation = false;

        Luau::Subtyping subtyping(
            builtins,
            Luau::NotNull{&arena},
            Luau::NotNull{&normalizer},
            Luau::NotNull{&runtime},
            Luau::NotNull{&reporter}
        );

        subtyping.limits = checking;
        Luau::Scope comparison_scope(builtins->emptyTypePack);
        Luau::CloneState clones{builtins};

        const auto covers = [&](Luau::TypeId broad, Luau::TypeId narrow) {
            budget.check();
            const auto *left = Luau::get<Luau::TableType>(broad);
            const auto *right = Luau::get<Luau::TableType>(narrow);

            if (left && right &&
                (left->name != right->name || left->definitionModuleName != right->definitionModuleName ||
                    left->definitionLocation != right->definitionLocation ||
                    left->instantiatedTypeParams != right->instantiatedTypeParams ||
                    left->instantiatedTypePackParams != right->instantiatedTypePackParams)) {
                return false;
            }

            if (Luau::is<
                    Luau::LazyType,
                    Luau::FreeType,
                    Luau::BlockedType,
                    Luau::PendingExpansionType,
                    Luau::TypeFunctionInstanceType,
                    Luau::ErrorType>(broad) ||
                Luau::is<
                    Luau::LazyType,
                    Luau::FreeType,
                    Luau::BlockedType,
                    Luau::PendingExpansionType,
                    Luau::TypeFunctionInstanceType,
                    Luau::ErrorType>(narrow)) {
                return false;
            }

            const auto subtype = Luau::clone(narrow, arena, clones);
            const auto supertype = Luau::clone(broad, arena, clones);

            if (clones.seenTypes.size() + clones.seenTypePacks.size() > limits.nodes) {
                return false;
            }

            for (const auto &[original, copied] : clones.seenTypes) {
                budget.check();

                if (Luau::is<
                        Luau::LazyType,
                        Luau::GenericType,
                        Luau::FreeType,
                        Luau::BlockedType,
                        Luau::PendingExpansionType,
                        Luau::TypeFunctionInstanceType,
                        Luau::ErrorType>(original)) {
                    return false;
                }
            }

            budget.check();
            const auto proof = subtyping.isSubtype(subtype, supertype, Luau::NotNull{&comparison_scope});
            budget.check();

            return proof.isSubtype && !proof.normalizationTooComplex && !proof.isErrorSuppressing &&
                   proof.errors.empty() && proof.assumedConstraints.empty() && proof.genericBoundsMismatches.empty();
        };

        std::vector<Luau::TypeId> reduced;

        for (const auto member : members) {
            bool is_covered = false;

            for (auto iterator = reduced.begin(); iterator != reduced.end();) {
                if (++visits > limits.edges) {
                    return members;
                }

                if (covers(*iterator, member)) {
                    is_covered = true;
                    break;
                }

                if (covers(member, *iterator)) {
                    iterator = reduced.erase(iterator);
                } else {
                    ++iterator;
                }
            }

            if (!is_covered) {
                reduced.push_back(member);
            }
        }

        return reduced;
    }
}
