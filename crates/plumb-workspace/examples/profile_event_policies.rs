use plumb_workspace::DiskWorkspace;
use std::{path::PathBuf, time::Instant};
fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("workspace root"));
    let mut loaded = DiskWorkspace::load(&root, None).unwrap();
    let path = root.join(
        std::env::args()
            .nth(2)
            .expect("document path relative to root"),
    );
    let source = std::fs::read_to_string(&path).unwrap();
    loaded.workspace.open_document(&path, 1, source.clone());
    let mut category_state = plumb_workspace::CategoryCheckState::default();
    let mut timeline_state = plumb_workspace::TimelineCheckState::default();
    let extra_path = root.join("__policy_benchmark_events.plumb");
    let target_path = root.join("__policy_benchmark_target.plumb");
    assert!(!extra_path.exists() && !target_path.exists());
    let extra = "`- 2026-09-22T10:00:00Z--13:00 `->{__policy_benchmark_target.plumb#item}\n `+ event\n`- 2026-09-22T10:30:00Z--11:00 `->{__policy_benchmark_target.plumb#item}\n `+ event\n`- 2026-09-22T11:00:00Z--12:00 `->{__policy_benchmark_target.plumb#item}\n `+ event\n";
    for phase in ["opened", "edited", "nested", "interval", "target"] {
        if phase == "nested" {
            loaded.workspace.open_document(
                &target_path,
                1,
                "`- Item\n `@ item\n `= event-category work\n",
            );
            loaded.workspace.open_document(&extra_path, 1, extra);
        }
        if phase == "interval" {
            loaded
                .workspace
                .open_document(&extra_path, 2, extra.replace("10:30", "10:40"));
        }
        if phase == "target" {
            loaded.workspace.open_document(
                &target_path,
                2,
                "`- Item\n `@ item\n `= event-category personal\n",
            );
        }
        if phase == "edited" {
            loaded
                .workspace
                .open_document(&path, 2, format!("\n{source}"));
        }
        for i in 0..3 {
            let t = Instant::now();
            let c = loaded
                .workspace
                .check_event_categories_incremental(&root, loaded.now, &mut category_state)
                .unwrap();
            println!(
                "{phase} category {i} ms={:.3} checked={} missing={} issues={} complete={}",
                t.elapsed().as_secs_f64() * 1000.,
                c.checked,
                c.missing.len(),
                c.issues.len(),
                c.complete
            );
            println!(
                "category extracted={} recomputed={} propagated={}",
                category_state.extracted_events,
                category_state.recomputed_events,
                category_state.dependency_propagations
            );
            let fresh = loaded
                .workspace
                .check_event_categories(&root, loaded.now, None)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&c).unwrap(),
                serde_json::to_value(fresh).unwrap()
            );
            let t = Instant::now();
            let c = loaded
                .workspace
                .check_event_timeline_incremental(&root, loaded.now, &mut timeline_state)
                .unwrap();
            println!("{phase} timeline {i} ms={:.3} checked={} gaps={} overlaps={} issues={} complete={}",t.elapsed().as_secs_f64()*1000.,c.checked,c.gaps.len(),c.overlaps.len(),c.issues.len(),c.complete);
            println!(
                "timeline extracted={} recomputed={} visited_intervals={}",
                timeline_state.extracted_events,
                timeline_state.recomputed_segments,
                timeline_state.visited_intervals
            );
            let fresh = loaded
                .workspace
                .check_event_timeline(&root, loaded.now)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&c).unwrap(),
                serde_json::to_value(fresh).unwrap()
            );
        }
    }
}
