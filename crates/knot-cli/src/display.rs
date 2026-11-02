use knot_core::{
    Change, DependencyEdge, DependencyRequirement, GraphReport, ObjectId, RequirementStatus, Status,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};

fn short(id: &ObjectId) -> String {
    id.to_string().chars().take(12).collect()
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn text(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

pub fn status(out: &mut impl Write, status: &Status) -> io::Result<()> {
    match (&status.branch, &status.head) {
        (Some(branch), Some(head)) => {
            writeln!(out, "On branch {} · HEAD {}", text(branch), short(head))?;
        }
        (Some(branch), None) => writeln!(out, "On branch {} · no commits yet", text(branch))?,
        (_, Some(head)) => writeln!(out, "Detached HEAD {}", short(head))?,
        _ => writeln!(out, "No commits yet")?,
    }
    if status.recovery_required {
        writeln!(
            out,
            "WARNING: interrupted operation; run knot recover --help"
        )?;
    }
    writeln!(out)?;
    if status.staged.is_empty() && status.unstaged.is_empty() && status.untracked.is_empty() {
        writeln!(out, "Working tree clean")?;
    } else {
        changes(out, "Staged changes", &status.staged)?;
        changes(out, "Unstaged changes", &status.unstaged)?;
        if !status.untracked.is_empty() {
            writeln!(out, "Untracked files:")?;
            for path in &status.untracked {
                writeln!(out, "  ?  {}", text(path))?;
            }
        }
    }
    writeln!(out)?;
    graph_inner(out, &status.dependency_graph, Some(status))?;
    if !status.unstaged.is_empty() {
        writeln!(
            out,
            "WARNING: unstaged edits are not included in dependency checks; stage them with knot add."
        )?;
    }
    Ok(())
}

fn changes(out: &mut impl Write, label: &str, changes: &[Change]) -> io::Result<()> {
    if !changes.is_empty() {
        writeln!(out, "{label}:")?;
        for change in changes {
            writeln!(out, "  {:8} {}", text(&change.kind), text(&change.path))?;
        }
    }
    Ok(())
}

pub fn graph(out: &mut impl Write, graph: &GraphReport) -> io::Result<()> {
    graph_inner(out, graph, None)
}

fn requirement(edge: &DependencyEdge) -> String {
    let required = match &edge.requirement {
        DependencyRequirement::Revision { revision } => format!("requires @{}", short(revision)),
        DependencyRequirement::Interface { name, interface } => {
            format!("requires {}@{}", text(name), short(interface))
        }
    };
    let state = match edge.status {
        RequirementStatus::Satisfied => "matches",
        RequirementStatus::MissingAsset => "MISSING ASSET",
        RequirementStatus::RevisionMismatch => "REVISION MISMATCH",
        RequirementStatus::MissingInterface => "MISSING INTERFACE",
        RequirementStatus::InterfaceMismatch => "INTERFACE MISMATCH",
    };
    let selected = if edge.status == RequirementStatus::InterfaceMismatch {
        edge.selected_interface
            .as_ref()
            .map(|id| format!("; selected interface @{}", short(id)))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(" — {required} [{state}]{selected}")
}

// Iterative traversal avoids recursion limits on deep engineering dependency graphs.
enum Step<'a> {
    Node {
        index: usize,
        prefix: String,
        last: Option<bool>,
        via: Option<&'a DependencyEdge>,
    },
    Missing {
        edge: &'a DependencyEdge,
        prefix: String,
        last: bool,
    },
    Leave(usize),
}

fn branch(last: bool) -> &'static str {
    if last { "└── " } else { "├── " }
}

fn graph_inner(
    out: &mut impl Write,
    graph: &GraphReport,
    status: Option<&Status>,
) -> io::Result<()> {
    let view = if status.is_some() {
        "staged; parent depends on child"
    } else {
        "parent depends on child"
    };
    writeln!(out, "Dependency graph ({view})")?;
    let by_asset: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (&node.asset, index))
        .collect();
    let mut outgoing: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut targets = BTreeSet::new();
    for edge in &graph.edges {
        outgoing.entry(&edge.dependent).or_default().push(edge);
        targets.insert(&edge.target);
    }
    for edges in outgoing.values_mut() {
        edges.sort_by(|a, b| {
            (&a.target_path, &a.target, &a.id).cmp(&(&b.target_path, &b.target, &b.id))
        });
    }
    let roots = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| !targets.contains(&node.asset))
        .map(|(index, _)| index)
        // Remaining components include cycles, which have no natural root.
        .chain(0..graph.nodes.len());
    let staged: BTreeSet<_> = status
        .into_iter()
        .flat_map(|s| &s.staged)
        .map(|c| c.path.as_str())
        .collect();
    let unstaged: BTreeMap<_, _> = status
        .into_iter()
        .flat_map(|s| &s.unstaged)
        .map(|c| (c.path.as_str(), c.kind.as_str()))
        .collect();
    let mut seen = BTreeSet::new();
    let mut active = BTreeSet::new();
    for root in roots {
        if seen.contains(&root) {
            continue;
        }
        let mut stack = vec![Step::Node {
            index: root,
            prefix: "  ".into(),
            last: None,
            via: None,
        }];
        while let Some(step) = stack.pop() {
            match step {
                Step::Leave(index) => {
                    active.remove(&index);
                }
                Step::Missing { edge, prefix, last } => writeln!(
                    out,
                    "{prefix}{}missing asset {}{}",
                    branch(last),
                    short(&edge.target),
                    requirement(edge)
                )?,
                Step::Node {
                    index,
                    prefix,
                    last,
                    via,
                } => {
                    let connector = last.map(branch).unwrap_or("");
                    let note = via.map(requirement).unwrap_or_default();
                    if !seen.insert(index) {
                        let reason = if active.contains(&index) {
                            "cycle"
                        } else {
                            "already shown"
                        };
                        writeln!(out, "{prefix}{connector}↳ #{} ({reason}){note}", index + 1)?;
                        continue;
                    }
                    let node = &graph.nodes[index];
                    let mut flags = String::new();
                    if staged.contains(node.path.as_str()) {
                        flags.push_str(" [staged]");
                    }
                    if let Some(kind) = unstaged.get(node.path.as_str()) {
                        flags.push_str(&format!(" [unstaged {kind}]"));
                    }
                    writeln!(
                        out,
                        "{prefix}{connector}#{} {} @{}{}{note}",
                        index + 1,
                        text(&node.path),
                        short(&node.revision),
                        flags
                    )?;
                    active.insert(index);
                    stack.push(Step::Leave(index));
                    let next_prefix = match last {
                        None => prefix,
                        Some(true) => format!("{prefix}    "),
                        Some(false) => format!("{prefix}│   "),
                    };
                    if let Some(edges) = outgoing.get(&node.asset) {
                        for (position, edge) in edges.iter().enumerate().rev() {
                            let last = position + 1 == edges.len();
                            stack.push(if let Some(&index) = by_asset.get(&edge.target) {
                                Step::Node {
                                    index,
                                    prefix: next_prefix.clone(),
                                    last: Some(last),
                                    via: Some(edge),
                                }
                            } else {
                                Step::Missing {
                                    edge,
                                    prefix: next_prefix.clone(),
                                    last,
                                }
                            });
                        }
                    }
                }
            }
        }
    }
    if graph.nodes.is_empty() {
        writeln!(
            out,
            "  No tracked files. Use knot add <path> to stage files."
        )?;
    }
    let conflicts = graph
        .edges
        .iter()
        .filter(|e| e.status != RequirementStatus::Satisfied)
        .count();
    writeln!(
        out,
        "\n{} · {} · {}",
        counted(graph.nodes.len(), "tracked file", "tracked files"),
        counted(graph.edges.len(), "dependency", "dependencies"),
        counted(conflicts, "conflict", "conflicts")
    )?;
    if conflicts > 0 {
        writeln!(
            out,
            "WARNING: unsatisfied requirements block ordinary commits."
        )?;
    }
    writeln!(
        out,
        "Requirements are declarations; manual verification is not yet recorded."
    )
}
