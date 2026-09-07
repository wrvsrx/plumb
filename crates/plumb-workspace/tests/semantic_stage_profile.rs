#![cfg(feature = "profile-semantic-stages")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use plumb_semantics::profiling::{lifetime_probe, root_owner_count, SemanticStages};
use plumb_semantics::{AnchorRecord, DocumentOutput, EventRecord, LinkRecord, TaskRecord};
use plumb_syntax::{Diagnostic, SourceChange};
use plumb_workspace::Workspace;

const PATH: &str = "profile.plumb";

fn fixture(count: usize) -> String {
    let mut source =
        "`= title Stage profile\n`= date 2026-09-07\n`= timezone +08:00\n\n".to_owned();
    for index in 0..count {
        if index % 20 == 0 {
            source.push_str(&format!("`# Section {index}\n `@ section-{index}\n\n"));
        }
        if index % 40 == 0 {
            source.push_str(&format!("`: term-{index} Definition body\n\n`table\n `- name age\n  `+ header\n `- Alice 10\n\n"));
        }
        source.push_str(&format!("`- Task {index}\n `+ task\n `@ task-{index}\n `= due 2026-09-08T10:00:00Z\n\n`- 10:00 Event {index}\n `+ event\n `= tasks #task-{index}\n\n See `->{{label #task-{index}}}\n\nBody {index}\n\n"));
    }
    source
}

fn edit(source: &str, count: usize, equal: bool) -> (String, SourceChange) {
    let needle = format!("{} {}", if equal { "Body" } else { "Event" }, count / 2);
    let replacement = if equal { "Text" } else { "Later" };
    let start = source.find(&needle).unwrap();
    let mut changed = source.to_owned();
    changed.replace_range(start..start + replacement.len(), replacement);
    (
        changed,
        SourceChange {
            old_range: start..start + replacement.len(),
            new_range: start..start + replacement.len(),
        },
    )
}

type Projection = (
    Vec<AnchorRecord>,
    Vec<LinkRecord>,
    Vec<TaskRecord>,
    Vec<EventRecord>,
    Vec<Diagnostic>,
);

// Owned absolute records used by reference/task/event and diagnostic consumers; not LSP JSON.
fn project(output: &DocumentOutput) -> Projection {
    (
        output.anchors().iter().collect(),
        output.links().iter().collect(),
        output.tasks().tasks.iter().collect(),
        output.events().events.iter().collect(),
        output.diagnostics().iter().collect(),
    )
}

fn isolated_output(mut workspace: Workspace) -> DocumentOutput {
    let entry = workspace.remove(PATH).unwrap();
    let output = Arc::clone(&entry.current.as_ref().unwrap().output);
    drop(entry);
    drop(workspace);
    let output = Arc::try_unwrap(output).expect("no external DocumentOutput Arc remains");
    assert_eq!(
        root_owner_count(&output),
        1,
        "no cloned DocumentOutput shares this root"
    );
    output
}

#[test]
fn profiled_production_analysis_matches_normal_including_short_circuit() {
    let source = fixture(8);
    for equal in [false, true] {
        let (changed, change) = edit(&source, 8, equal);
        let mut normal = Workspace::new();
        normal.open_document(PATH, 1, &source);
        let mut profiled = normal.clone();
        let plain = normal
            .begin_document_revision_with_change(PATH, 2, &changed, Some(change.clone()))
            .unwrap()
            .analyze();
        let (timed, _) = profiled
            .begin_document_revision_with_change(PATH, 2, &changed, Some(change))
            .unwrap()
            .analyze_profiled();
        assert_eq!(
            normal.install_document_analysis_with_impact(plain),
            profiled.install_document_analysis_with_impact(timed)
        );
        let normal = isolated_output(normal);
        let profiled = isolated_output(profiled);
        assert_eq!(normal, profiled);
        assert_eq!(project(&normal), project(&profiled));
        assert_eq!(profiled.reused_document_reducers(), equal);
    }
    let mut normal = Workspace::new();
    let mut profiled = Workspace::new();
    normal.open_document(PATH, 1, "{invalid\n");
    profiled.open_document(PATH, 1, "{invalid\n");
    let plain = normal
        .begin_document_revision(PATH, 2, &source)
        .unwrap()
        .analyze();
    let (timed, _) = profiled
        .begin_document_revision(PATH, 2, &source)
        .unwrap()
        .analyze_profiled();
    assert_eq!(
        normal.install_document_analysis_with_impact(plain),
        profiled.install_document_analysis_with_impact(timed)
    );
    assert_eq!(isolated_output(normal), isolated_output(profiled));
}

#[test]
fn destruction_releases_internal_roots_and_distinguishes_shared_old_nodes() {
    for retain_old in [false, true] {
        for equal in [false, true] {
            let source = fixture(8);
            let (changed, change) = edit(&source, 8, equal);
            let mut workspace = Workspace::new();
            workspace.open_document(PATH, 1, source);
            let retained = retain_old.then(|| workspace.clone());
            let (prepared, _) = workspace
                .begin_document_revision_with_change(PATH, 2, changed, Some(change))
                .unwrap()
                .analyze_profiled();
            assert!(workspace.install_document_analysis(prepared));
            let output = isolated_output(workspace);
            let shared = output.reused_semantic_node_count() + output.semantic_equal_node_count();
            let probe = lifetime_probe(&output);
            let root_clone = output.clone();
            assert_eq!(root_owner_count(&output), 2);
            drop(output);
            assert!(
                !probe.root_tree_syntax_released(),
                "outer handle drop is not destruction"
            );
            drop(root_clone);
            assert!(probe.root_tree_syntax_released());
            assert_eq!(
                probe.live_local_nodes(),
                if retain_old { shared } else { 0 }
            );
            drop(retained);
            assert_eq!(probe.live_local_nodes(), 0);
        }
    }
}

#[derive(Default)]
struct Sample {
    syntax: Duration,
    semantic: Duration,
    stages: SemanticStages,
    publication: Duration,
    projection: Duration,
    projection_drop: Duration,
    detach: Duration,
    semantic_drop: Duration,
    total: Duration,
}

fn run(
    source: &str,
    changed: &str,
    change: &SourceChange,
    profiled: bool,
    retain_old: bool,
) -> Sample {
    let mut workspace = Workspace::new();
    workspace.open_document(PATH, 1, source);
    let retained = retain_old.then(|| workspace.clone());
    let next_source = changed.to_owned();
    let mut sample = Sample::default();
    let total = Instant::now();
    let started = Instant::now();
    let pending = workspace
        .begin_document_revision_with_change(PATH, 2, next_source, Some(change.clone()))
        .unwrap();
    sample.syntax = started.elapsed();
    let started = Instant::now();
    let prepared = if profiled {
        let (prepared, stages) = pending.analyze_profiled();
        sample.stages = stages;
        prepared
    } else {
        pending.analyze()
    };
    sample.semantic = started.elapsed();
    assert!(
        sample.stages.metadata_context
            + sample.stages.local_nodes
            + sample.stages.document_reduction
            <= sample.semantic
    );
    let started = Instant::now();
    let impact = workspace
        .install_document_analysis_with_impact(prepared)
        .unwrap();
    sample.publication = started.elapsed();
    let started = Instant::now();
    let projection = project(
        &workspace
            .get(PATH)
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output,
    );
    sample.projection = started.elapsed();
    let started = Instant::now();
    drop(std::hint::black_box(projection));
    sample.projection_drop = started.elapsed();
    let started = Instant::now();
    drop(std::hint::black_box(impact));
    let output = isolated_output(workspace);
    sample.detach = started.elapsed();
    let started = Instant::now();
    drop(std::hint::black_box(output));
    sample.semantic_drop = started.elapsed();
    sample.total = total.elapsed();
    // The retained-old case excludes old-revision teardown from the new-revision pipeline.
    drop(retained);
    sample
}

#[test]
#[ignore = "manual release production stage profile"]
fn profile_production_semantic_stages() {
    use sha2::Digest;
    let source = fixture(2000);
    {
        let mut validation = Workspace::new();
        let entry = validation.open_document(PATH, 1, &source);
        assert!(entry.parsed.is_valid());
        let output = &entry.current.as_ref().unwrap().output;
        assert_eq!(output.tasks().tasks.len(), 2000);
        assert_eq!(output.events().events.len(), 2000);
        assert_eq!(output.links().len(), 2000);
        assert_eq!(output.anchors().len(), 2100);
        assert_eq!(output.tables().tables.len(), 50);
    }
    eprintln!("fixture=mixed-2000 bytes={} sha256={:x} warmup=5 samples=30 units=us order=alternating-pairs; old initial build/source clone excluded", source.len(), sha2::Sha256::digest(source.as_bytes()));
    for equal in [false, true] {
        let (changed, change) = edit(&source, 2000, equal);
        for retain_old in [false, true] {
            let mut groups = [Vec::new(), Vec::new()];
            for iteration in 0..35 {
                let order = if iteration % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                };
                for profiled in order {
                    let sample = run(&source, &changed, &change, profiled, retain_old);
                    if iteration >= 5 {
                        groups[usize::from(profiled)].push(sample);
                    }
                }
            }
            for (profiled, samples) in [false, true].into_iter().zip(groups) {
                eprintln!("equal={equal} retain_old={retain_old} profiled={profiled}");
                for (name, get) in [
                    (
                        "syntax_revision_and_state_switch",
                        (|s: &Sample| s.syntax) as fn(&Sample) -> Duration,
                    ),
                    ("metadata_context", |s: &Sample| s.stages.metadata_context),
                    ("local_nodes_and_bookkeeping", |s: &Sample| {
                        s.stages.local_nodes
                    }),
                    ("document_reduction", |s: &Sample| {
                        s.stages.document_reduction
                    }),
                    ("semantic_total", |s: &Sample| s.semantic),
                    ("workspace_install_with_impact", |s: &Sample| s.publication),
                    ("owned_absolute_projection", |s: &Sample| s.projection),
                    ("projection_destruction", |s: &Sample| s.projection_drop),
                    ("workspace_detach", |s: &Sample| s.detach),
                    ("semantic_destruction", |s: &Sample| s.semantic_drop),
                    ("complete_pipeline", |s: &Sample| s.total),
                ] {
                    if !profiled
                        && matches!(
                            name,
                            "metadata_context"
                                | "local_nodes_and_bookkeeping"
                                | "document_reduction"
                        )
                    {
                        continue;
                    }
                    let mut values = samples.iter().map(get).collect::<Vec<_>>();
                    values.sort();
                    eprintln!(
                        "  {name}: p50={:.3} p95={:.3}",
                        values[14].as_secs_f64() * 1e6,
                        values[28].as_secs_f64() * 1e6
                    );
                }
            }
        }
    }
}
