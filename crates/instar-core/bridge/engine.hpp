#pragma once

#include <cstddef>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace instar {

    struct Position {
        unsigned line;
        unsigned column;
    };

    struct Range {
        Position begin;
        Position end;
    };

    struct Module {
        std::string name;
        std::string source;
    };

    struct Configuration {
        std::string module;
        std::string path;
        std::string source;
    };

    struct RobloxClass {
        std::string name;
        bool service;
        bool creatable;
    };

    struct RobloxNode {
        std::string name;
        std::string class_name;
        std::optional<std::size_t> parent;
    };

    struct RobloxMetadata {
        std::vector<std::string> enumerations;
        std::vector<RobloxClass> classes;
        std::vector<RobloxNode> nodes;
    };

    using ModuleReader = std::function<std::optional<std::string>(const std::string &)>;
    using ModuleResolver = std::function<std::optional<std::string>(const std::string &, Range, std::string_view)>;

    enum class Mode {
        Strict,
        Nonstrict,
        NoCheck,
    };

    enum class Solver {
        New,
        Old,
    };

    struct Diagnostic {
        std::string path;
        std::string message;
        Range range;
        bool error;
    };

    struct Alias {
        std::string name;
        std::string value;
    };

    struct AliasResult {
        std::vector<Alias> aliases;
        std::optional<std::string> error;
    };

    struct TypeInformation {
        std::string description;
    };

    struct Completion {
        std::string label;
        std::string description;
        std::string documentation;
        std::string insert_text;
        unsigned kind;
        bool deprecated;
    };

    struct Signature {
        std::string description;
        std::vector<std::string> parameters;
        unsigned active_parameter;
    };

    struct Destination {
        std::string path;
        Range range;
        std::string name;
    };

    struct Call {
        Destination target;
        Range caller;
        std::optional<Range> container;
    };

    struct Annotation {
        std::string path;
        Position position;
        std::string text;
    };

    class Engine final {
        public:
        explicit Engine(std::vector<Module> modules = {}, ModuleReader reader = {}, ModuleResolver resolver = {},
            std::vector<Configuration> configurations = {}, Solver solver = Solver::New,
            std::optional<Mode> mode = std::nullopt);

        ~Engine();

        Engine(Engine &&) noexcept;
        Engine &operator=(Engine &&) noexcept;

        Engine(const Engine &) = delete;
        Engine &operator=(const Engine &) = delete;

        void update(std::vector<Module> modules, std::vector<Configuration> configurations = {});
        void load_definitions(std::vector<Module> definitions, std::optional<std::vector<std::string>> target_paths);
        void prepare_roblox(const RobloxMetadata &metadata);
        std::vector<Diagnostic> check();
        std::optional<TypeInformation> type_at(const std::string &path, Position position);
        std::optional<TypeInformation> hover(const std::string &path, Position position);
        std::vector<Completion> complete(const std::string &path, Position position);
        std::optional<Signature> signature(const std::string &path, Position position);
        std::optional<Destination> definition(const std::string &path, Position position);
        std::optional<Destination> type_definition(const std::string &path, Position position);
        std::vector<Destination> implementations(const std::string &path, Position position);
        std::vector<Destination> references(const std::string &path, Position position);
        std::vector<Annotation> annotations(const std::string &path);
        std::vector<Call> calls(const std::string &path);

        private:
        struct Implementation;
        std::unique_ptr<Implementation> implementation;
    };

    AliasResult parse_aliases(const std::string &source, bool executable);

} // namespace instar

struct NativeBytes {
    const char *data;
    std::size_t size;
};

struct NativePosition {
    unsigned line;
    unsigned column;
};

struct NativeRange {
    NativePosition begin;
    NativePosition end;
};

struct NativeModule {
    NativeBytes name;
    NativeBytes source;
};

struct NativeConfiguration {
    NativeBytes module;
    NativeBytes path;
    NativeBytes source;
};

struct NativeRobloxClass {
    NativeBytes name;
    int service;
    int creatable;
};

struct NativeRobloxNode {
    NativeBytes name;
    NativeBytes class_name;
    std::size_t parent;
    int has_parent;
};

enum NativeMode {
    NativeModeConfigured,
    NativeModeStrict,
    NativeModeNonstrict,
    NativeModeNoCheck,
};

using NativeModuleReader = NativeBytes (*)(void *, NativeBytes);
using NativeModuleResolver = NativeBytes (*)(void *, NativeBytes, NativeRange, NativeBytes);
using NativeReport = void (*)(void *, NativeBytes, NativeBytes, NativeRange, int);
using NativeType = void (*)(void *, NativeBytes);
using NativeCompletion = void (*)(void *, NativeBytes, NativeBytes, NativeBytes, NativeBytes, unsigned, int);
using NativeSignature = void (*)(void *, NativeBytes, const NativeBytes *, std::size_t, unsigned);
using NativeDestination = void (*)(void *, NativeBytes, NativeRange, NativeBytes);
using NativeCall = void (*)(void *, NativeBytes, NativeRange, NativeBytes, NativeRange, int, NativeRange);
using NativeAnnotation = void (*)(void *, NativeBytes, NativePosition, NativeBytes);
using NativeAlias = void (*)(void *, NativeBytes, NativeBytes);
using NativeFailure = void (*)(void *, NativeBytes);

extern "C" {
    void *instar_engine_create(const NativeModule *, std::size_t, void *, NativeModuleReader, NativeModuleResolver,
        const NativeConfiguration *, std::size_t, int, int);

    void instar_engine_destroy(void *);

    int instar_engine_load_definitions(
        void *, const NativeModule *, std::size_t, const NativeBytes *, std::size_t, int, void *, NativeFailure);

    int instar_engine_prepare_roblox(void *, const NativeBytes *, std::size_t, const NativeRobloxClass *, std::size_t,
        const NativeRobloxNode *, std::size_t, void *, NativeFailure);

    void instar_engine_check(void *, void *, NativeReport, NativeFailure);
    void instar_engine_type_at(void *, NativeBytes, NativePosition, void *, NativeType, NativeFailure);
    void instar_engine_hover(void *, NativeBytes, NativePosition, void *, NativeType, NativeFailure);
    void instar_engine_complete(void *, NativeBytes, NativePosition, void *, NativeCompletion, NativeFailure);
    void instar_engine_signature(void *, NativeBytes, NativePosition, void *, NativeSignature, NativeFailure);
    void instar_engine_definition(void *, NativeBytes, NativePosition, void *, NativeDestination, NativeFailure);
    void instar_engine_type_definition(void *, NativeBytes, NativePosition, void *, NativeDestination, NativeFailure);
    void instar_engine_implementations(void *, NativeBytes, NativePosition, void *, NativeDestination, NativeFailure);
    void instar_engine_references(void *, NativeBytes, NativePosition, void *, NativeDestination, NativeFailure);
    void instar_engine_annotations(void *, NativeBytes, void *, NativeAnnotation, NativeFailure);
    void instar_engine_calls(void *, NativeBytes, void *, NativeCall, NativeFailure);
    void instar_parse_aliases(NativeBytes, int, void *, NativeAlias, NativeFailure);
    int instar_matches(NativeBytes, NativeBytes);
}
