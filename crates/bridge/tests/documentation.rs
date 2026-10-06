//! Native documentation namespace and declaration identity contracts.

use instar_analysis::{Completion, Options};

use instar_bridge::{
    Configuration,
    frontend::{Definition, Frontend},
};

use std::{io, time::Duration};

#[test]
fn namespaces_preserve_builtins_members_and_file_diagnostics() -> io::Result<()> {
    let mut frontend = Frontend::new()?;
    frontend.insert("entry", "return 1", 1, &Configuration::new()?, &[])?;
    let options = Options::new(Duration::from_secs(5));

    let mut definitions = vec![
        Definition {
            name: "types.d.luau".to_owned(),
            namespace: "@game".to_owned(),
            revision: 1,
            text: "declare extern type Thing with\n function Method(self): number\nend".to_owned(),
        },
        Definition {
            name: "globals.d.luau".to_owned(),
            namespace: "@game".to_owned(),
            revision: 1,
            text: "declare thing: Thing\ndeclare library: { call: () -> number }".to_owned(),
        },
    ];

    frontend.definitions("entry", &definitions)?;

    for (path, expected) in [
        ("global/library.call", "@game/global/library.call"),
        ("globaltype/Thing.Method", "@game/globaltype/Thing.Method"),
        ("global/thing", "@game/global/thing"),
        ("global/type", "@luau/global/type"),
    ] {
        assert_eq!(
            frontend.documentation("entry", path, &options)?.as_deref(),
            Some(expected)
        );
    }

    for definition in &mut definitions {
        definition.namespace = "@renamed".to_owned();
    }

    frontend.definitions("entry", &definitions)?;

    assert_eq!(
        frontend
            .documentation("entry", "global/library.call", &options)?
            .as_deref(),
        Some("@renamed/global/library.call")
    );

    definitions[1].text = "declare broken: UnknownType".to_owned();
    definitions[1].revision = 2;
    frontend.definitions("entry", &definitions)?;
    let result = frontend.check(&["entry".to_owned()], &options)?;
    assert_ne!(result.completion, Completion::Complete);
    assert_ne!(result.diagnostics, Vec::new());

    assert!(
        result
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.location.module == "globals.d.luau"
                && diagnostic.location.revision == 2)
    );

    Ok(())
}

#[test]
fn declaration_identity_is_shared_across_distinct_namespaces() -> io::Result<()> {
    let mut frontend = Frontend::new()?;
    let configuration = Configuration::new()?;
    let options = Options::new(Duration::from_secs(5));

    for (module, namespace) in [("first", "@first"), ("second", "@second")] {
        frontend.insert(module, "return 1", 1, &configuration, &[])?;

        frontend.definitions(
            module,
            &[Definition {
                name: "shared.d.luau".to_owned(),
                namespace: namespace.to_owned(),
                revision: 1,
                text: "declare value: number".to_owned(),
            }],
        )?;
    }

    for (module, expected) in [
        ("first", "@first/global/value"),
        ("second", "@second/global/value"),
    ] {
        assert_eq!(
            frontend
                .documentation(module, "global/value", &options)?
                .as_deref(),
            Some(expected)
        );
    }

    Ok(())
}
