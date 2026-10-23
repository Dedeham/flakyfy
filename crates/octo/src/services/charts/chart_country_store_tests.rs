use super::*;

fn charts(country: &str, pinned: &[&str]) -> ListenerCharts {
    ListenerCharts {
        country: country.to_string(),
        pinned: pinned.iter().map(|c| (*c).to_string()).collect(),
    }
}

#[test]
fn a_choice_survives_a_reload_from_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chart-countries.json");
    ChartCountryStore::new(Some(path.clone())).set("anna", charts("br", &["jp", "kr"]));

    let again = ChartCountryStore::new(Some(path.clone()));

    assert_eq!(again.get("anna"), charts("br", &["jp", "kr"]));
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved,
        serde_json::json!({"anna": {"country": "br", "pinned": ["jp", "kr"]}})
    );
}

#[test]
fn choosing_nothing_removes_the_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chart-countries.json");
    let store = ChartCountryStore::new(Some(path.clone()));
    store.set("anna", charts("br", &[]));
    store.set("bob", charts("", &["jp"]));

    store.set("anna", ListenerCharts::default());

    assert_eq!(store.get("anna"), ListenerCharts::default());
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(saved.get("anna").is_none());
    assert!(saved.get("bob").is_some());
}

#[test]
fn names_ignore_case() {
    let store = ChartCountryStore::new(None);
    store.set(" Anna ", charts("br", &[]));

    assert_eq!(store.get("anna").country, "br");
    assert_eq!(store.get("ANNA").country, "br");
    assert_eq!(store.all().len(), 1);
    assert_eq!(store.all()[0].0, "anna");
}

#[test]
fn countries_hold_homes_and_pins_once_each() {
    let store = ChartCountryStore::new(None);
    store.set("anna", charts("br", &["jp", "kr"]));
    store.set("bob", charts("jp", &["br"]));
    store.set("cara", charts("", &["de"]));

    assert_eq!(store.countries(), vec!["br", "de", "jp", "kr"]);
}

#[test]
fn a_broken_file_is_moved_aside_and_the_store_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chart-countries.json");
    std::fs::write(&path, b"{ not json").unwrap();

    let store = ChartCountryStore::new(Some(path.clone()));

    assert!(store.all().is_empty());
    assert!(!path.exists(), "moved aside");
    let aside: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("chart-countries.json.broken-"))
        })
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(std::fs::read(&aside[0]).unwrap(), b"{ not json", "kept as it was");
    store.set("anna", charts("br", &[]));
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["anna"]["country"], "br");
}

#[test]
fn a_file_in_any_case_is_read_in_lowercase() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chart-countries.json");
    std::fs::write(
        &path,
        br#"{"Anna": {"country": " BR ", "pinned": ["JP", "jp", "Kr"]}, "bob": {"pinned": ["De"]}}"#,
    )
    .unwrap();

    let store = ChartCountryStore::new(Some(path));

    assert_eq!(store.get("anna"), charts("br", &["jp", "kr"]));
    assert_eq!(store.get("BOB"), charts("", &["de"]));
    assert_eq!(store.countries(), vec!["br", "de", "jp", "kr"]);
}

#[test]
fn saves_from_many_threads_leave_the_newest_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chart-countries.json");
    let store = std::sync::Arc::new(ChartCountryStore::new(Some(path.clone())));

    let threads: Vec<_> = (0..8)
        .map(|n| {
            let store = std::sync::Arc::clone(&store);
            std::thread::spawn(move || {
                for i in 0..20 {
                    store.set(
                        &format!("user{n}"),
                        charts(if i % 2 == 0 { "br" } else { "jp" }, &[]),
                    );
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }

    let again = ChartCountryStore::new(Some(path));
    assert_eq!(again.all(), store.all(), "the file holds the last choices");
}

#[test]
fn a_refused_change_leaves_the_choices_as_they_were() {
    let store = ChartCountryStore::new(None);
    store.set("anna", charts("br", &["jp"]));

    let refused = store.update("anna", |mine| {
        mine.pinned.push("kr".into());
        Err("no")
    });
    let added = store.update("anna", |mine| {
        mine.pinned.push("kr".into());
        Ok::<(), ()>(())
    });

    assert_eq!(refused, Err("no"));
    assert_eq!(added, Ok(charts("br", &["jp", "kr"])));
    assert_eq!(store.get("anna"), charts("br", &["jp", "kr"]));
}
