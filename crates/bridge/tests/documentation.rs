//! Native documentation namespace and declaration identity contracts.
use std::{io, time::Duration};

use instar_analysis::{Completion, Options};

use instar_bridge::{
    Configuration,
    frontend::{Definition, Frontend},
};

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

#[test]
fn shared_declaration_revisions_update_atomically() -> io::Result<()> {
    let mut frontend = Frontend::new()?;
    let configuration = Configuration::new()?;
    let options = Options::new(Duration::from_secs(5));

    for module in ["first", "second"] {
        frontend.insert(
            module,
            "local result: string = value\nreturn result",
            1,
            &configuration,
            &[],
        )?;

        frontend.definitions(
            module,
            &[Definition {
                name: "shared".to_owned(),
                namespace: format!("@{module}"),
                revision: 1,
                text: "declare value: number".to_owned(),
            }],
        )?;
    }

    let mut replacement = Definition {
        name: "shared".to_owned(),
        namespace: "@first".to_owned(),
        revision: 2,
        text: "declare value: string".to_owned(),
    };

    frontend.definitions("first", &[replacement.clone()])?;

    for module in ["first", "second"] {
        let checked = frontend.check(&[module.to_owned()], &options)?;
        assert_eq!(checked.completion, Completion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

        assert_eq!(
            frontend.documentation(module, "global/value", &options)?,
            Some(format!("@{module}/global/value"))
        );
    }

    replacement.text = "declare value: number".to_owned();

    assert!(
        frontend
            .definitions("first", &[replacement.clone()])
            .is_err()
    );

    replacement.revision = 1;
    assert!(frontend.definitions("first", &[replacement]).is_err());
    let checked = frontend.check(&["second".to_owned()], &options)?;
    assert_eq!(checked.diagnostics, Vec::new());

    Ok(())
}

#[test]
fn documentation_preserves_interruption_and_environment_error_categories() -> io::Result<()> {
    let mut frontend = Frontend::new()?;
    frontend.insert("entry", "return 1", 1, &Configuration::new()?, &[])?;
    let cancelled = Options::new(Duration::from_secs(5));
    cancelled.cancellation.cancel();

    assert_eq!(
        frontend
            .documentation("entry", "global/print", &cancelled)
            .expect_err("cancelled")
            .kind(),
        io::ErrorKind::Interrupted
    );

    assert_eq!(
        frontend
            .documentation("entry", "global/print", &Options::new(Duration::ZERO))
            .expect_err("timed out")
            .kind(),
        io::ErrorKind::TimedOut
    );

    frontend.definitions(
        "entry",
        &[Definition {
            name: "invalid".to_owned(),
            namespace: "@custom".to_owned(),
            revision: 1,
            text: "declare value: UnknownType".to_owned(),
        }],
    )?;

    assert_eq!(
        frontend
            .documentation(
                "entry",
                "global/value",
                &Options::new(Duration::from_secs(5))
            )
            .expect_err("invalid environment")
            .kind(),
        io::ErrorKind::InvalidData
    );

    Ok(())
}

#[test]
fn unrelated_type_errors_do_not_hide_builtin_documentation() -> io::Result<()> {
    let mut frontend = Frontend::new()?;

    frontend.insert(
        "entry",
        "--!strict\nlocal value: number = 'wrong'\nreturn value",
        1,
        &Configuration::new()?,
        &[],
    )?;

    let options = Options::new(Duration::from_secs(5));

    assert_ne!(
        frontend.check(&["entry".to_owned()], &options)?.diagnostics,
        Vec::new()
    );

    assert_eq!(
        frontend.documentation("entry", "global/print", &options)?,
        Some("@luau/global/print".to_owned())
    );

    Ok(())
}
