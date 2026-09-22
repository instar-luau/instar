//! Temporary human-readable inspector for reachable require graphs.

use std::{
    env,
    error::Error,
    io::{self, Write},
    path::{Path, PathBuf},
};

use instar_core::{graph::Graph, project::Project};

fn main() -> Result<(), Box<dyn Error>> {
    let paths = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    if paths.len() == 1 && matches!(paths[0].to_str(), Some("--help" | "-h")) {
        println!(
            "Usage: inspect <entry.lua[u]>...\nPrints modules, require targets, unresolved reasons, and cycle groups."
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
    modules.sort_by_key(|&id| &graph.nodes[id].module.source);

    for id in modules {
        let node = &graph.nodes[id];

        let marker = if graph.entries.contains(&id) {
            " [entry]"
        } else {
            ""
        };

        writeln!(
            output,
            "\n{}{marker}",
            display_path(&node.module.source, base).display()
        )?;

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
                writeln!(
                    output,
                    " -> {}",
                    display_path(&graph.nodes[target].module.source, base).display()
                )?;
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
                    .map(|&id| &graph.nodes[id].module.source)
                    .collect::<Vec<_>>();

                paths.sort();

                paths
            })
            .collect::<Vec<_>>();

        groups.sort();

        for (index, paths) in groups.iter().enumerate() {
            writeln!(output, "  Group {}:", index + 1)?;

            for path in paths {
                writeln!(output, "    {}", display_path(path, base).display())?;
            }
        }
    }

    Ok(())
}
