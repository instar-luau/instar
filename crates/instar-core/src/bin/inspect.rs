//! Temporary human-readable inspector for reachable require graphs.

use std::{
    env,
    error::Error,
    io::{self, Write},
    path::{Path, PathBuf},
};

use instar_core::{graph::Graph, project::Project, resolve::Module};

fn main() -> Result<(), Box<dyn Error>> {
    let paths = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    if paths.len() == 1 && matches!(paths[0].to_str(), Some("--help" | "-h")) {
        println!(
            "Usage: inspect <entry.lua[u]>...\nPrints modules, require targets, unresolved reasons, and cycle groups.\nRoblox defaults to the nearest ancestor sourcemap.json; override with [roblox].sourcemaps in instar.toml ([] disables discovery)."
        );

        return Ok(());
    }

    if paths.is_empty() {
        return Err("at least one entry file is required".into());
    }

    let mut project = Project::new();
    let mut graph = Graph::new();
    graph.add_entries(&mut project, &paths)?;

    let mut stdout = io::BufWriter::new(io::stdout().lock());
    render(&mut stdout, &graph, &env::current_dir()?)?;
    stdout.flush()?;

    Ok(())
}

fn display_path<'path>(path: &'path Path, base: &Path) -> &'path Path {
    path.strip_prefix(base).unwrap_or(path)
}

fn module_key(module: &Module) -> (&Path, Option<(&Path, &str)>) {
    (
        &module.source,
        module
            .instance
            .as_ref()
            .map(|instance| (instance.sourcemap_path(), instance.full_name())),
    )
}

fn write_module(output: &mut impl Write, module: &Module, base: &Path) -> io::Result<()> {
    write!(output, "{}", display_path(&module.source, base).display())?;

    if let Some(instance) = &module.instance {
        write!(
            output,
            " [{}; place: {}]",
            instance.full_name(),
            display_path(instance.sourcemap_path(), base).display()
        )?;
    }

    Ok(())
}

fn render(output: &mut impl Write, graph: &Graph, base: &Path) -> io::Result<()> {
    let unresolved = graph
        .nodes
        .node_weights()
        .flat_map(|node| &node.requires)
        .filter(|site| site.failure.is_some())
        .count();

    writeln!(
        output,
        "{} modules, {} dependencies, {unresolved} unresolved requires",
        graph.nodes.node_count(),
        graph.nodes.edge_count()
    )?;

    let mut modules = graph.nodes.node_indices().collect::<Vec<_>>();
    modules.sort_by_key(|&id| module_key(&graph.nodes[id].module));

    for id in modules {
        let node = &graph.nodes[id];

        let marker = if graph.entries.contains(&id) {
            " [entry]"
        } else {
            ""
        };

        writeln!(output)?;
        write_module(output, &node.module, base)?;
        writeln!(output, "{marker}")?;

        for diagnostic in &node.diagnostics {
            if let Some(offset) = diagnostic.offset {
                writeln!(output, "  ERROR at byte {offset}: {}", diagnostic.message)?;
            } else {
                writeln!(output, "  ERROR: {}", diagnostic.message)?;
            }
        }

        if node.requires.is_empty() && node.diagnostics.is_empty() {
            writeln!(output, "  (no requires)")?;
        }

        for site in &node.requires {
            let expression = site.expression.replace('\r', "\\r").replace('\n', "\\n");

            write!(
                output,
                "  {}:{} require({expression})",
                site.location[0], site.location[1]
            )?;

            if let Some(target) = site.target {
                write!(output, " -> ")?;
                write_module(output, &graph.nodes[target].module, base)?;
                writeln!(output)?;
            } else if let Some(failure) = &site.failure {
                writeln!(output, " -> UNRESOLVED: {failure}")?;
            }
        }
    }

    let cycles = graph.cycles();

    if !cycles.is_empty() {
        writeln!(output, "\nCycle groups (mutually reachable modules):")?;

        let mut groups = cycles
            .iter()
            .map(|component| {
                let mut paths = component
                    .iter()
                    .map(|&id| &graph.nodes[id].module)
                    .collect::<Vec<_>>();

                paths.sort_by_key(|module| module_key(module));

                paths
            })
            .collect::<Vec<_>>();

        groups.sort_by(|left, right| {
            left.iter()
                .map(|module| module_key(module))
                .cmp(right.iter().map(|module| module_key(module)))
        });

        for (index, paths) in groups.iter().enumerate() {
            writeln!(output, "  Group {}:", index + 1)?;

            for path in paths {
                write!(output, "    ")?;
                write_module(output, path, base)?;
                writeln!(output)?;
            }
        }
    }

    Ok(())
}
