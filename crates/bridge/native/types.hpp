#ifndef INSTAR_LUAU_NATIVE_TYPES
#define INSTAR_LUAU_NATIVE_TYPES

#include "Luau/Cancellation.h"
#include "Luau/Scope.h"
#include "Luau/ToString.h"
#include "instar-bridge/src/bridge.rs.h"

#include <memory>
#include <string>
#include <vector>

namespace instar::luau {
    struct PresentationBudget {
        explicit PresentationBudget(std::shared_ptr<Luau::FrontendCancellationToken> cancellation);

        std::shared_ptr<Luau::FrontendCancellationToken> cancellation;

        void check() const;
    };

    struct CompletionType {
        Luau::TypeId type;
        std::string text;
        bool structural;
    };

    class Printer {
    public:
        Printer(PresentationBudget &budget, Luau::ScopePtr scope);

        std::string type(Luau::TypeId value, bool expanded = false);
        std::string pack(Luau::TypePackId value, bool expanded = false, bool returns = false);
        std::string function(const Luau::FunctionType &value, const std::string &name, bool expanded);
        std::string signature(Luau::TypeId value);
        NativePresentation presentation(Luau::TypeId value, bool expanded);
        NativePresentation presentation(Luau::TypePackId value, bool expanded);

        std::size_t equivalence(Luau::TypeId value, const std::string &text, std::vector<CompletionType> &types);

        std::string finish(std::string value);
        bool complete = true;

    private:
        PresentationBudget &budget;
        Luau::ToStringOptions options;
    };
}

#endif
