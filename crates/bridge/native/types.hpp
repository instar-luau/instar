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
        bool is_structural;
    };

    class Printer {
    public:
        Printer(PresentationBudget &budget, Luau::ScopePtr scope);

        std::string type(Luau::TypeId value, bool is_expanded = false);
        std::string pack(Luau::TypePackId value, bool is_expanded = false, bool is_return_pack = false);
        std::string function(const Luau::FunctionType &value, const std::string &name, bool is_expanded);
        std::string signature(Luau::TypeId value);
        NativePresentation presentation(Luau::TypeId value, bool is_expanded);
        NativePresentation presentation(Luau::TypePackId value, bool is_expanded);
        void abbreviate(NativePresentation &presentation);

        std::size_t equivalence(Luau::TypeId value, const std::string &text, std::vector<CompletionType> &types);

        std::string finish(std::string value);
        bool is_complete = true;

    private:
        template <typename Formatter> std::string render(bool is_expanded, Formatter formatter);

        PresentationBudget &budget;
        Luau::ToStringOptions options;
    };
}

#endif
