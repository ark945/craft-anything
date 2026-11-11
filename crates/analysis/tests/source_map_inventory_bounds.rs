use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, analyze_application};
use craft_anything_analysis::js_source::{SourceGraph, build_source_graph, diff_source_and_bundle};
use serde_json::{Value, json};

fn graph_with_maps(maps: Vec<Value>) -> AppGraph {
    let mut files = Vec::new();
    for (i, map) in maps.into_iter().enumerate() {
        files.push(AppFile { path: format!("script-{i}.js"), bytes: format!("//# sourceMappingURL=script-{i}.js.map\n").into_bytes() });
        files.push(AppFile { path: format!("script-{i}.js.map"), bytes: serde_json::to_vec(&map).unwrap() });
    }
    let listing = files.iter().map(|file| (file.path.clone(), file.bytes.len() as u64)).collect();
    analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap()
}

fn source_graph(paths: &[&str]) -> SourceGraph {
    let files: Vec<_> = paths.iter().map(|path| ((*path).to_string(), Vec::new())).collect();
    build_source_graph("reference", &files, 0, false, &Default::default(), &|| false).unwrap()
}

#[test]
fn application_inventory_reports_the_entry_limit() {
    let sources: Vec<_> = (0..5001).map(|i| format!("source-{i}.js")).collect();
    let graph = graph_with_maps(vec![json!({"version": 3, "sourceRoot": "src", "sources": sources, "mappings": ""})]);
    let map = &graph.source_maps[0];
    assert_eq!(map.sources.len(), 5000);
    assert_eq!(map.sources.first().map(String::as_str), Some("src/source-0.js"));
    assert_eq!(map.sources.last().map(String::as_str), Some("src/source-4999.js"));
    assert!(map.sources_truncated);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("script-0.js")));
}

#[test]
fn expanded_path_bytes_are_bounded_across_application_maps() {
    let maps = ["first", "second"]
        .into_iter()
        .map(|prefix| {
            json!({
                "version": 3, "sourceRoot": "r".repeat(2 * 1024 * 1024),
                "sources": (0..20).map(|i| format!("/{prefix}-{i}.js")).collect::<Vec<_>>(), "mappings": ""
            })
        })
        .collect();
    let graph = graph_with_maps(maps);
    assert_eq!(graph.source_maps.len(), 2);
    assert_eq!(graph.source_maps[0].sources.len(), 20);
    assert!(!graph.source_maps[0].sources_truncated);
    assert_eq!(graph.source_maps[1].sources.len(), 11);
    assert!(graph.source_maps[1].sources_truncated);
    let bytes: usize = graph.source_maps.iter().flat_map(|map| &map.sources).map(String::len).sum();
    assert!(bytes <= 64 * 1024 * 1024);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("script-1.js")));

    let sources = source_graph(&["first-0.js", "second-0.js", "second-19.js", "not-listed.js", "script-1.js"]);
    let diff = diff_source_and_bundle(&sources, &graph, 100);
    assert_eq!(diff["unmatched_sources"], json!([]), "omitted sources are not confirmed absent: {diff}");
    assert_eq!(
        diff["uncertain_sources"],
        json!([
            {"source": "second-19.js", "reason": "source_map_inventory_truncated"},
            {"source": "not-listed.js", "reason": "source_map_inventory_truncated"}
        ])
    );
    assert_eq!(diff["summary"]["uncertain_sources"], 2);
    assert_eq!(diff["summary"]["unmatched_sources"], 0);
    assert_eq!(diff["summary"]["matched"], 3);
    assert_eq!(diff["matches"][0]["unit"], "script-0.js");
    assert_eq!(diff["matches"][1]["unit"], "script-1.js");
    assert_eq!(diff["matches"][2]["quality"], "strong");
    assert_eq!(diff["source_map_inventory_truncated"], true);

    let first_row = diff_source_and_bundle(&sources, &graph, 1);
    assert_eq!(first_row["summary"]["matched"], 1);
    assert_eq!(first_row["uncertain_sources"], json!([]));
    assert_eq!(first_row["source_map_inventory_truncated"], true, "positive matches do not make the inventory complete");
}

#[test]
fn ordinary_inventories_keep_paths_content_counts_and_complete_evidence() {
    let graph = graph_with_maps(vec![json!({
        "version": 3, "sourceRoot": "src", "sources": ["a.js", null, "/b.js"],
        "sourcesContent": ["first();", null, "second();"], "mappings": ""
    })]);
    let map = &graph.source_maps[0];
    assert_eq!(map.sources, ["src/a.js", "src//b.js"]);
    assert_eq!(map.sources_with_content, 2);
    assert!(!map.sources_truncated);
    assert!(!graph.limitations.iter().any(|(key, _)| key == "source_map_inventory_truncated"));
    let mut saved = serde_json::to_value(&graph).unwrap();
    saved["source_maps"][0].as_object_mut().unwrap().remove("sources_truncated");
    let restored: AppGraph = serde_json::from_value(saved).unwrap();
    assert!(!restored.source_maps[0].sources_truncated);
    assert_eq!(restored.source_maps[0].sources, map.sources);

    for app in [graph, restored] {
        let diff = diff_source_and_bundle(&source_graph(&["src/a.js", "src/missing.js"]), &app, 100);
        assert_eq!(diff["summary"]["matched"], 1);
        assert_eq!(diff["unmatched_sources"], json!(["src/missing.js"]));
        assert_eq!(diff["summary"]["unmatched_sources"], 1);
        assert_eq!(diff["uncertain_sources"], json!([]));
        assert_eq!(diff["summary"]["uncertain_sources"], 0);
        assert_eq!(diff["source_map_inventory_truncated"], false);
    }
}

#[test]
fn missing_sources_without_maps_remain_unmatched() {
    let diff = diff_source_and_bundle(&source_graph(&["src/missing.js"]), &graph_with_maps(Vec::new()), 100);
    assert_eq!(diff["unmatched_sources"], json!(["src/missing.js"]));
    assert_eq!(diff["summary"]["unmatched_sources"], 1);
    assert_eq!(diff["uncertain_sources"], json!([]));
    assert_eq!(diff["summary"]["uncertain_sources"], 0);
    assert_eq!(diff["source_map_inventory_truncated"], false);
}

#[test]
fn a_map_refused_for_too_many_entries_leaves_its_sources_uncertain() {
    // More names than the parser keeps by default: the map is refused, not read as empty.
    let names = vec!["\"\""; craft_anything_formats::sourcemap::Limits::default().max_entries + 1].join(",");
    let map = format!("{{\"version\":3,\"sources\":[\"a.js\"],\"mappings\":\"\",\"names\":[{names}]}}").into_bytes();
    let files = vec![
        AppFile { path: "script-0.js".into(), bytes: b"//# sourceMappingURL=script-0.js.map\n".to_vec() },
        AppFile { path: "script-0.js.map".into(), bytes: map },
    ];
    let listing = files.iter().map(|file| (file.path.clone(), file.bytes.len() as u64)).collect();
    let graph = analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap();
    let map = &graph.source_maps[0];
    assert!(map.present);
    assert!(map.sources.is_empty());
    assert!(map.sources_truncated);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("script-0.js")));
    let diff = diff_source_and_bundle(&source_graph(&["a.js"]), &graph, 100);
    assert_eq!(diff["unmatched_sources"], json!([]), "a refused map confirms nothing absent: {diff}");
    assert_eq!(diff["uncertain_sources"], json!([{"source": "a.js", "reason": "source_map_inventory_truncated"}]));
}

#[test]
fn a_map_refused_at_another_limit_leaves_its_sources_uncertain() {
    // More generated lines than the parser keeps by default.
    let lines = ";".repeat(craft_anything_formats::sourcemap::Limits::default().max_lines + 1);
    let graph = graph_with_maps(vec![json!({"version": 3, "sources": ["a.js"], "mappings": lines})]);
    assert!(graph.source_maps[0].sources_truncated);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("generated lines")));
    let diff = diff_source_and_bundle(&source_graph(&["a.js"]), &graph, 100);
    assert_eq!(diff["uncertain_sources"], json!([{"source": "a.js", "reason": "source_map_inventory_truncated"}]));
    // A malformed map is unchanged: its sources are still read as absent.
    let broken = graph_with_maps(vec![json!({"version": 2, "sources": ["a.js"], "mappings": ""})]);
    assert!(!broken.source_maps[0].sources_truncated);
}
