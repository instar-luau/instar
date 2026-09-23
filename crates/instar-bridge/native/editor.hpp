#ifndef EDITOR_HPP
#define EDITOR_HPP

#include "bridge.hpp"

#ifdef __cplusplus
#include "Luau/Frontend.h"
#endif

#ifdef __cplusplus
extern "C" {
#endif
    /**Hover information for one source position.*/
    typedef struct EditorHover {
        /**Symbol name.*/
        Text name;
        /**Displayed type.*/
        Text type;
        /**Documentation symbol identifier.*/
        Text documentation;
        /**Whether a range is present.*/
        uint8_t has_range;
        /**Hover range when present.*/
        Location range;
        /**Whether the selected symbol is a named type alias.*/
        uint8_t is_type;
    } EditorHover;

    /**Completion source mode.*/
    typedef enum EditorCompletionMode {
        /**Normal expression completion.*/
        CompletionNormal = 0,
        /**Import completion.*/
        CompletionImport = 1,
        /**Require-path completion.*/
        CompletionRequirePath = 2,
    } EditorCompletionMode;

    /**Completion item kind.*/
    typedef enum EditorCompletionKind {
        /**Plain text completion.*/
        CompletionText = 0,
        /**Method completion.*/
        CompletionMethod = 1,
        /**Function completion.*/
        CompletionFunction = 2,
        /**Constructor completion.*/
        CompletionConstructor = 3,
        /**Field completion.*/
        CompletionField = 4,
        /**Variable completion.*/
        CompletionVariable = 5,
        /**Class completion.*/
        CompletionClass = 6,
        /**Interface completion.*/
        CompletionInterface = 7,
        /**Module completion.*/
        CompletionModule = 8,
        /**Property completion.*/
        CompletionProperty = 9,
        /**Unit completion.*/
        CompletionUnit = 10,
        /**Value completion.*/
        CompletionValue = 11,
        /**Enum completion.*/
        CompletionEnum = 12,
        /**Keyword completion.*/
        CompletionKeyword = 13,
        /**Snippet completion.*/
        CompletionSnippet = 14,
        /**Color completion.*/
        CompletionColor = 15,
        /**File completion.*/
        CompletionFile = 16,
        /**Reference completion.*/
        CompletionReference = 17,
        /**Folder completion.*/
        CompletionFolder = 18,
        /**Enum-member completion.*/
        CompletionEnumMember = 19,
        /**Constant completion.*/
        CompletionConstant = 20,
        /**Struct completion.*/
        CompletionStruct = 21,
        /**Event completion.*/
        CompletionEvent = 22,
        /**Operator completion.*/
        CompletionOperator = 23,
        /**Type-parameter completion.*/
        CompletionTypeParameter = 24,
    } EditorCompletionKind;

    /**Identity used to resolve a rename.*/
    typedef enum EditorRenameKind {
        /**Lexically scoped variable.*/
        RenameLocal = 0,
        /**Source-defined global variable.*/
        RenameGlobal = 1,
        /**Table or class property.*/
        RenameProperty = 2,
        /**Type alias.*/
        RenameType = 3,
    } EditorRenameKind;

    /**One completion item.*/
    typedef struct EditorCompletionItem {
        /**Completion label.*/
        Text name;
        /**Completion detail.*/
        Text detail;
        /**Documentation symbol identifier.*/
        Text documentation;
        /**Completion insertion text.*/
        Text insert;
        /**Whether a range is present.*/
        uint8_t has_range;
        /**Completion range when present.*/
        Location range;
        /**Completion source mode.*/
        EditorCompletionMode mode;
        /**Completion item kind.*/
        EditorCompletionKind kind;
        /**Whether the item is deprecated.*/
        uint8_t deprecated;
        /**Source module containing the declaration, empty when unavailable.*/
        Text definition_module;
        /**Source declaration range.*/
        Location definition;
    } EditorCompletionItem;

    /**Signature help for one call site.*/
    typedef struct EditorSignatureHelp {
        /**Signature label.*/
        Text label;
        /**Signature parameter labels.*/
        const Text *parameters;
        /**Number of parameters.*/
        size_t parameter_count;
        /**Whether an active parameter is present.*/
        uint8_t has_active_parameter;
        /**Active parameter index.*/
        uint32_t active_parameter;
    } EditorSignatureHelp;

    /**Navigation target and selection range.*/
    typedef struct EditorNavigation {
        /**Target source path.*/
        Text path;
        /**Full target range.*/
        Location range;
        /**Name-selection range.*/
        Location selection;
    } EditorNavigation;

    /**One reference occurrence.*/
    typedef struct EditorReference {
        /**Reference source path.*/
        Text path;
        /**Reference range.*/
        Location range;
        /**Whether the reference is a declaration.*/
        uint8_t declaration;
    } EditorReference;
    /**Symbol identity information used to select conservative workspace candidates.*/
    typedef struct EditorReferenceTarget {
        /**Selected symbol name.*/
        Text name;
        /**Whether occurrences are confined to the declaration module.*/
        uint8_t local;
        /**Whether the selected symbol is a property.*/
        uint8_t property;
    } EditorReferenceTarget;

    /**Resolved, source-editable rename target.*/
    typedef struct EditorRenameTarget {
        /**Current name.*/
        Text name;
        /**Declaration module.*/
        Text path;
        /**Declaration name range.*/
        Location definition;
        /**Selected occurrence range.*/
        Location selection;
        /**Target identity category.*/
        EditorRenameKind kind;
    } EditorRenameTarget;

    /**Inferred type for one source range.*/
    typedef struct EditorTypeHint {
        /**Type-hint range.*/
        Location range;
        /**Inferred type text.*/
        Text type;
        /**Argument name, empty for variable type hints.*/
        Text parameter;
    } EditorTypeHint;

    /**Receives hover information.*/
    typedef uint8_t (*HoverCallback)(void *context, const EditorHover *hover);
    /**Receives one completion item.*/
    typedef uint8_t (*CompletionCallback)(void *context, const EditorCompletionItem *item);
    /**Receives signature help.*/
    typedef uint8_t (*SignatureCallback)(void *context, const EditorSignatureHelp *signature);
    /**Receives one navigation target.*/
    typedef uint8_t (*NavigationCallback)(void *context, const EditorNavigation *navigation);
    /**Receives one reference occurrence.*/
    typedef uint8_t (*ReferenceCallback)(void *context, const EditorReference *reference);
    /**Receives selected symbol identity information.*/
    typedef uint8_t (*ReferenceTargetCallback)(void *context, const EditorReferenceTarget *target);
    /**Receives a resolved rename target.*/
    typedef uint8_t (*RenameTargetCallback)(void *context, const EditorRenameTarget *target);
    /**Receives one type hint.*/
    typedef uint8_t (*HintCallback)(void *context, const EditorTypeHint *hint);

    /**Returns hover information at a source position.*/
    int32_t editor_hover(void *checker, Text name, uint32_t line, uint32_t column, HoverCallback callback, void *context, String *error);
    /**Returns completion items at a source position.*/
    int32_t editor_completion(void *checker, Text name, uint32_t line, uint32_t column, CompletionCallback callback, void *context, String *error);
    /**Returns signature help at a source position.*/
    int32_t editor_signature_help(void *checker, Text name, uint32_t line, uint32_t column, SignatureCallback callback, void *context, String *error);
    /**Returns inferred type hints for a module.*/
    int32_t editor_type_hints(void *checker, Text name, HintCallback callback, void *context, String *error);
    /**Finds the definition at a source position.*/
    int32_t editor_definition(void *checker, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error);
    /**Finds the declaration at a source position.*/
    int32_t editor_declaration(void *checker, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error);
    /**Finds concrete source implementations at a source position.*/
    int32_t editor_implementation(void *checker, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error);
    /**Finds the type definition at a source position.*/
    int32_t editor_type_definition(void *checker, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error);
    /**Finds references at a source position.*/
    int32_t editor_references(void *checker, Text name, uint32_t line, uint32_t column, const Text *candidates, size_t candidate_count, ReferenceCallback callback, void *context, String *error);
    /**Resolves the semantic symbol at a source position for reference candidate selection.*/
    int32_t editor_reference_target(void *checker, Text name, uint32_t line, uint32_t column, ReferenceTargetCallback callback, void *context, String *error);
    /**Resolves a source-editable rename target.*/
    int32_t editor_rename_target(void *checker, Text name, uint32_t line, uint32_t column, RenameTargetCallback callback, void *context, String *error);
    /**Validates a rename and returns every affected occurrence.*/
    int32_t editor_rename(void *checker, Text name, uint32_t line, uint32_t column, Text new_name, const Text *candidates, size_t candidate_count, ReferenceCallback callback, void *context, String *error);
#ifdef __cplusplus
}

namespace instar {
    int32_t editor_hover(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, HoverCallback callback, void *context);
    int32_t editor_completion(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, CompletionCallback callback, void *context);
    int32_t editor_signature_help(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, SignatureCallback callback, void *context);
    int32_t editor_type_hints(Luau::Frontend &frontend, Text name, HintCallback callback, void *context);
    int32_t editor_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context);
    int32_t editor_declaration(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context);
    int32_t editor_implementation(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context);
    int32_t editor_type_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context);
    int32_t editor_references(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, const Text *candidates, size_t candidate_count, ReferenceCallback callback, void *context);
    int32_t editor_reference_target(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, ReferenceTargetCallback callback, void *context);
    int32_t editor_rename_target(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, RenameTargetCallback callback, void *context);
    int32_t editor_rename(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, Text new_name, const Text *candidates, size_t candidate_count, ReferenceCallback callback, void *context);
} // namespace instar
#endif

#endif
