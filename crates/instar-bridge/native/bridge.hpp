#ifndef BRIDGE_HPP
#define BRIDGE_HPP

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif
    /**Borrowed UTF-8 byte sequence passed across the native ABI.*/
    typedef struct Text {
        /**Pointer to UTF-8 bytes.*/
        const uint8_t *data;
        /**Number of bytes.*/
        size_t length;
    } Text;

    /**Owned UTF-8 byte sequence returned by the native ABI.*/
    typedef struct String {
        /**Pointer to owned UTF-8 bytes.*/
        uint8_t *data;
        /**Number of bytes.*/
        size_t length;
    } String;

    /**Half-open source range using zero-based line and column coordinates.*/
    typedef struct Location {
        /**Inclusive start line.*/
        uint32_t begin_line;
        /**Inclusive start column.*/
        uint32_t begin_column;
        /**Exclusive end line.*/
        uint32_t end_line;
        /**Exclusive end column.*/
        uint32_t end_column;
    } Location;

    /**Result code returned by native operations.*/
    typedef enum Status {
        /**Operation succeeded.*/
        StatusSuccess = 0,
        /**Operation failed.*/
        StatusFailure = 1,
        /**A callback rejected the operation.*/
        StatusCallbackFailure = 2,
    } Status;

    /**Kind of source returned by a source callback.*/
    typedef enum SourceKind {
        /**Source kind is unknown.*/
        SourceUnknown = 0,
        /**Source is a module.*/
        SourceModule = 1,
        /**Source is a script.*/
        SourceScript = 2,
    } SourceKind;

    /**Options used when constructing a checker.*/
    typedef struct FrontendOptions {
        /**Retain complete type graphs.*/
        uint8_t retain_full_type_graphs;
        /**Configure the checker for autocomplete.*/
        uint8_t for_autocomplete;
        /**Run lint checks.*/
        uint8_t run_lint_checks;
    } FrontendOptions;

    /**Options used when loading a definition source.*/
    typedef struct DefinitionOptions {
        /**Capture comments while loading definitions.*/
        uint8_t capture_comments;
        /**Type-check definitions for autocomplete.*/
        uint8_t type_check_for_autocomplete;
    } DefinitionOptions;

    /**Additional source location associated with a diagnostic.*/
    typedef struct RelatedDiagnostic {
        /**Related source path.*/
        Text path;
        /**Related source range.*/
        Location location;
        /**Related diagnostic message.*/
        Text message;
    } RelatedDiagnostic;

    /**Severity assigned to a diagnostic.*/
    typedef enum DiagnosticSeverity {
        /**Error diagnostic.*/
        DiagnosticError = 0,
        /**Warning diagnostic.*/
        DiagnosticWarning = 1,
        /**Informational diagnostic.*/
        DiagnosticInformation = 2,
    } DiagnosticSeverity;

    /**Diagnostic emitted by parsing, checking, or linting.*/
    typedef struct Diagnostic {
        /**Diagnostic source path.*/
        Text path;
        /**Diagnostic source range.*/
        Location location;
        /**Diagnostic severity.*/
        DiagnosticSeverity severity;
        /**Diagnostic message.*/
        Text message;
        /**Whether a related diagnostic is present.*/
        uint8_t has_related;
        /**Related diagnostic when present.*/
        RelatedDiagnostic related;
    } Diagnostic;

    /**Source location of a module-resolution expression.*/
    typedef struct ResolveRequest {
        /**Original source module being traced.*/
        Text from;
        /**Result of the preceding navigation step, when present.*/
        Text context;
        /**Whether an intermediate context is present.*/
        uint8_t has_context;
        /**Whether failure is allowed.*/
        uint8_t optional;
        /**Expression source range.*/
        Location expression;
    } ResolveRequest;

    /**Result returned by a resolution callback.*/
    typedef struct ResolveResult {
        /**Resolved source path.*/
        Text path;
        /**Whether a result is present.*/
        uint8_t present;
    } ResolveResult;

    /**Result returned by a source callback.*/
    typedef struct SourceResult {
        /**Source bytes.*/
        Text source;
        /**Source kind.*/
        SourceKind kind;
    } SourceResult;

    /**Supplies source bytes and kind for a module name.*/
    typedef uint8_t (*SourceCallback)(void *context, Text name, SourceResult *result);
    /**Supplies a configuration handle for a module name.*/
    typedef uint8_t (*ConfigurationCallback)(void *context, Text name, const void **configuration);
    /**Resolves a module or instance request.*/
    typedef uint8_t (*ResolveCallback)(void *context, const ResolveRequest *request, ResolveResult *result);
    /**Receives one diagnostic from the checker.*/
    typedef uint8_t (*DiagnosticCallback)(void *context, const Diagnostic *diagnostic);
    /**Receives one text item from an enumeration operation.*/
    typedef uint8_t (*ItemCallback)(void *context, Text item);

    /**Callbacks used by the native checker to access project data.*/
    typedef struct BridgeCallbacks {
        /**Source callback.*/
        SourceCallback source;
        /**Configuration callback.*/
        ConfigurationCallback configuration;
        /**Resolution callback.*/
        ResolveCallback resolve;
        /**Diagnostic callback.*/
        DiagnosticCallback diagnostic;
    } BridgeCallbacks;

    /**Releases an owned string returned by the native ABI.*/
    void string_destroy(String value);

    /**Creates a configuration handle from JSON source.*/
    void *configuration_create(Text source, String *error);
    /**Destroys a configuration handle.*/
    void configuration_destroy(void *configuration);

    /**Creates a checker handle.*/
    void *checker_create(const BridgeCallbacks *callbacks, void *context, const FrontendOptions *options, String *error);
    /**Destroys a checker handle.*/
    void checker_destroy(void *checker);
    /**Rebinds the callback context for one checker operation.*/
    int32_t checker_set_context(void *checker, void *context, String *error);
    /**Marks a module and its dependents dirty.*/
    int32_t checker_mark_dirty(void *checker, Text name, String *error);
    /**Clears ordinary source caches; rebuild the checker to change definitions or globals.*/
    int32_t checker_clear_sources(void *checker, String *error);
    /**Freezes the global type arena before parsing or checking.*/
    int32_t checker_freeze(void *checker, String *error);
    /**Loads one definition source into the checker.*/
    int32_t checker_load_definition(void *checker, Text source, Text package_name, const DefinitionOptions *options, String *error);
    /**Parses one module.*/
    int32_t checker_parse(void *checker, Text name, String *error);
    /**Emits parse diagnostics for one module.*/
    int32_t checker_parse_diagnostics(void *checker, Text name, String *error);
    /**Checks one module and emits its timeout module names.*/
    int32_t checker_check(void *checker, Text name, ItemCallback timeout_callback, void *timeout_context, String *error);

    /**Emits cached diagnostics and their timeout module names.*/
    int32_t
    checker_result(void *checker, Text name, uint8_t accumulate_nested, uint8_t for_autocomplete, ItemCallback timeout_callback, void *timeout_context, String *error);

    /**Enumerates global names known to the checker.*/
    int32_t checker_globals(void *checker, ItemCallback callback, void *context, String *error);
    /**Enumerates modules required by the checker.*/
    int32_t checker_modules(void *checker, ItemCallback callback, void *context, String *error);
    /**Attaches inferred type data to one module.*/
    int32_t checker_attach_type_data(void *checker, Text name, String *error);
#ifdef __cplusplus
}
#endif

#endif
