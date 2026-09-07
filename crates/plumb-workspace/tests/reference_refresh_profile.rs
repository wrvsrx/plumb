use plumb_workspace::{ExportedSemanticChange, Workspace};

#[test]
#[ignore = "manual reference-input installation profile"]
fn profile_anchor_link_reference_installation() {
    let mut anchors = String::new();
    for index in 0..2000 {
        anchors.push_str(&format!("`# Heading\n `@ anchor-{index}\n\n"));
    }
    anchors.push_str(" Tail\n");
    let links = "See `->{target.plumb#task}\n".repeat(2000);
    let mut metadata = String::new();
    for index in 0..2000 {
        metadata.push_str(&format!("`= field-{index} metadata scalar value\n"));
    }
    metadata.push_str("\nSee `->{foo.plumb}\n");
    for (name, old, new) in [
        (
            "large_metadata",
            metadata.clone(),
            metadata.replace("foo.plumb", "bar.plumb"),
        ),
        (
            "anchors",
            anchors.clone(),
            anchors.replace("Tail", "Longer tail"),
        ),
        (
            "links",
            links.clone(),
            links.replace("`->{target.plumb#task}", "`->\"target.plumb#task\""),
        ),
    ] {
        let mut base = Workspace::new();
        base.open_document("source.plumb", 1, old);
        let mut samples = Vec::new();
        for iteration in 0..55 {
            let mut workspace = base.clone();
            let prepared = workspace
                .begin_document_revision("source.plumb", 2, new.clone())
                .unwrap()
                .analyze();
            let started = std::time::Instant::now();
            let impact = workspace
                .install_document_analysis_with_impact(prepared)
                .unwrap();
            let elapsed = started.elapsed();
            assert_eq!(impact.exported, ExportedSemanticChange::Changed);
            std::hint::black_box(impact);
            if iteration >= 5 {
                samples.push(elapsed);
            }
        }
        samples.sort();
        eprintln!(
            "{name}: 2000 records installation p50={:?} p95={:?}",
            samples[24], samples[47]
        );
    }
}
