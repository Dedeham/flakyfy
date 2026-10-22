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
fn a_broken_file_starts_empty_and_stays_until_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chart-countries.json");
    std::fs::write(&path, b"{ not json").unwrap();

    let store = ChartCountryStore::new(Some(path.clone()));

    assert!(store.all().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    store.set("anna", charts("br", &[]));
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["anna"]["country"], "br");
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
