//! The places file: converting GeoNames rows, and looking up postal codes and cities in what
//! the converter wrote.

use std::path::{Path, PathBuf};

use super::*;

/// GeoNames rows, as `allCountries.txt` has them: country, postal code, place, admin name 1,
/// admin code 1, admin name 2, admin code 2, admin name 3, admin code 3, latitude, longitude,
/// accuracy.
pub(crate) const FIXTURE: &[&str] = &[
    "US\t90210\tBeverly Hills\tCalifornia\tCA\tLos Angeles\t037\t\t\t34.0901\t-118.4065\t4",
    "US\t02134\tAllston\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.3539\t-71.1337\t4",
    "US\t62701\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.8017\t-89.6436\t4",
    "US\t62702\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.8201\t-89.6437\t4",
    "US\t62703\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.7623\t-89.6271\t4",
    "US\t01101\tSpringfield\tMassachusetts\tMA\tHampden\t013\t\t\t42.1015\t-72.5898\t4",
    "US\t01103\tSpringfield\tMassachusetts\tMA\tHampden\t013\t\t\t42.1029\t-72.5887\t4",
    "GB\tSW1A\tLondon\tEngland\tENG\tGreater London\t11609024\t\t\t51.5\t-0.1397\t",
    "JP\t100-0001\tChiyoda\tTokyo To\t40\tChiyoda Ku\t1864529\t\t\t35.6895\t139.7536\t",
    "CH\t8001\tZürich\tKanton Zürich\tZH\tBezirk Zürich\t112\t\t\t47.3721\t8.5422\t1",
    "PL\t59-700\tRakowice\tLower Silesia\t72\t\t\t\t\t51.2447\t15.5347\t6",
    "PL\t59-700\tGolnice\tLower Silesia\t72\t\t\t\t\t51.3504\t15.5852\t6",
    "GB\tE16\tCanning Town\tEngland\tENG\t\t\t\t\t51.51\t0.01\t",
    "FJ\t0000\tTaveuni\t\t\t\t\t\t\t-16.8\t179.95\t",
    "CA\tT0A\tEastern Alberta (St. Paul)\tAlberta\tAB\t\t\t\t\t54.766\t-111.7174\t6",
];

/// The fixture rows as a places file in a temp folder.
pub(crate) fn fixture_file(dir: &Path) -> PathBuf {
    let path = dir.join("places.bin");
    write_places(&path, FIXTURE);
    path
}

/// `rows` converted into a places file at `path`.
pub(crate) fn write_places(path: &Path, rows: &[&str]) -> ConvertSummary {
    let text = rows.join("\n");
    let file = std::fs::File::create(path).expect("a places file");
    convert(text.as_bytes(), std::io::BufWriter::new(file)).expect("converted")
}

fn index() -> (tempfile::TempDir, PlaceIndex) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = fixture_file(dir.path());
    (dir, PlaceIndex::new(path))
}

fn place(code: &str, name: &str, state: &str, country: &str, latitude: f64, longitude: f64) -> Place {
    Place {
        code: code.into(),
        name: name.into(),
        state: state.into(),
        country: country.into(),
        latitude,
        longitude,
    }
}

// ---- Keys ----------------------------------------------------------------------------------

#[test]
fn codes_ignore_case_spaces_and_dashes_and_keep_leading_zeros() {
    assert_eq!(code_key("sw1a 1aa"), "SW1A1AA");
    assert_eq!(code_key(" SW1A1AA "), "SW1A1AA");
    assert_eq!(code_key("100-0001"), "1000001");
    assert_eq!(code_key("02134"), "02134");
}

#[test]
fn names_ignore_case_accents_and_punctuation() {
    assert_eq!(name_key("Zürich"), "zurich");
    assert_eq!(name_key("  WINSTON-SALEM "), "winston salem");
    assert_eq!(name_key("St. Paul"), "st paul");
}

#[test]
fn countries_are_two_letters() {
    assert_eq!(country_key(" us "), Some("US".into()));
    assert_eq!(country_key("USA"), None);
    assert_eq!(country_key("1A"), None);
    assert_eq!(country_key(""), None);
}

#[test]
fn coarser_codes_are_the_parts_the_data_keeps() {
    assert_eq!(coarser_codes("GB", "SW1A1AA"), vec!["SW1A"]);
    assert_eq!(coarser_codes("GB", "M11AE"), vec!["M1"]);
    assert_eq!(coarser_codes("CA", "T0A1A0"), vec!["T0A"]);
    assert_eq!(coarser_codes("NL", "1012AB"), vec!["1012"]);
    assert_eq!(coarser_codes("IE", "D02X285"), vec!["D02"]);
    assert_eq!(coarser_codes("AR", "C1425ABC"), vec!["1425"]);
    assert_eq!(coarser_codes("BR", "01310100"), vec!["01310000"]);
    assert!(coarser_codes("BR", "01310000").is_empty());
    assert!(coarser_codes("US", "90210").is_empty());
    assert!(coarser_codes("GB", "SW1A").is_empty());
}

#[test]
fn coordinates_round_to_two_decimals_without_minus_zero() {
    assert_eq!(round2(34.0901), 34.09);
    assert_eq!(round2(-118.4065), -118.41);
    assert_eq!(round2(-0.001), 0.0);
    assert!(round2(-0.001).is_sign_positive());
}

// ---- Postal codes --------------------------------------------------------------------------

#[test]
fn a_postal_code_is_found_with_its_place() {
    let (_dir, index) = index();
    assert_eq!(
        index.by_code("US", "90210"),
        Some(place(
            "90210",
            "Beverly Hills",
            "California",
            "US",
            34.09,
            -118.41
        ))
    );
    assert_eq!(
        index.by_code("us", " 90210 ").map(|p| p.name),
        Some("Beverly Hills".into())
    );
}

#[test]
fn leading_zeros_are_part_of_the_code() {
    let (_dir, index) = index();
    assert_eq!(index.by_code("US", "02134").map(|p| p.code), Some("02134".into()));
    assert_eq!(index.by_code("US", "2134"), None);
}

#[test]
fn a_code_written_with_a_dash_is_found_either_way_and_answered_as_written() {
    let (_dir, index) = index();
    for asked in ["100-0001", "1000001", "100 0001"] {
        assert_eq!(
            index.by_code("JP", asked),
            Some(place("100-0001", "Chiyoda", "Tokyo To", "JP", 35.69, 139.75)),
            "{asked}"
        );
    }
}

#[test]
fn a_full_uk_postcode_is_found_by_its_outward_code_in_any_spelling() {
    let (_dir, index) = index();
    let london = Some(place("SW1A", "London", "England", "GB", 51.5, -0.14));
    assert_eq!(index.by_code("GB", "sw1a 1aa"), london);
    assert_eq!(index.by_code("GB", "SW1A1AA"), london);
    assert_eq!(index.by_code("gb", "SW1A"), london);
}

#[test]
fn a_canadian_code_is_found_by_its_first_three() {
    let (_dir, index) = index();
    assert_eq!(index.by_code("CA", "t0a 1a0").map(|p| p.code), Some("T0A".into()));
}

#[test]
fn a_code_listed_twice_keeps_its_first_place() {
    let (_dir, index) = index();
    assert_eq!(
        index.by_code("PL", "59-700").map(|p| p.name),
        Some("Rakowice".into())
    );
}

#[test]
fn an_unknown_code_country_or_blank_code_finds_nothing() {
    let (_dir, index) = index();
    assert_eq!(index.by_code("US", "99999"), None);
    assert_eq!(index.by_code("DE", "90210"), None);
    assert_eq!(index.by_code("USA", "90210"), None);
    assert_eq!(index.by_code("US", " - "), None);
}

// ---- Cities --------------------------------------------------------------------------------

#[test]
fn a_city_without_a_state_is_the_one_with_the_most_postal_codes() {
    let (_dir, index) = index();
    let found = index.by_name("US", "springfield", None).expect("a city");
    assert_eq!(found.state, "Illinois");
    assert_eq!(found.code, "62701");
    // The mean of its three codes.
    assert_eq!((found.latitude, found.longitude), (39.79, -89.64));
}

#[test]
fn a_city_with_a_state_by_name_or_code() {
    let (_dir, index) = index();
    for state in ["MA", "ma", "Massachusetts", " massachusetts "] {
        let found = index.by_name("US", "Springfield", Some(state)).expect(state);
        assert_eq!(found.state, "Massachusetts", "{state}");
        assert_eq!(found.code, "01101");
        assert_eq!((found.latitude, found.longitude), (42.1, -72.59));
    }
    assert_eq!(index.by_name("US", "Springfield", Some("Texas")), None);
}

#[test]
fn a_city_is_found_without_its_accents() {
    let (_dir, index) = index();
    let found = index.by_name("ch", "zurich", None).expect("Zürich");
    assert_eq!(found, place("8001", "Zürich", "Kanton Zürich", "CH", 47.37, 8.54));
    assert_eq!(
        index.by_name("CH", "ZÜRICH", Some("zh")).map(|p| p.code),
        Some("8001".into())
    );
}

#[test]
fn a_city_name_is_matched_whole() {
    let (_dir, index) = index();
    assert_eq!(index.by_name("US", "Spring", None), None);
    assert_eq!(index.by_name("US", "Springfield Gardens", None), None);
    assert_eq!(index.by_name("US", "  ", None), None);
}

// ---- Coordinates -------------------------------------------------------------------------

#[test]
fn the_nearest_of_several_places_is_the_one_found() {
    let (_dir, index) = index();
    assert_eq!(index.nearest(39.8, -89.64).map(|p| p.code), Some("62701".into()));
    assert_eq!(
        index.nearest(39.816, -89.641).map(|p| p.code),
        Some("62702".into())
    );
    assert_eq!(index.nearest(39.75, -89.62).map(|p| p.code), Some("62703".into()));
    assert_eq!(
        index.nearest(34.1, -118.4),
        Some(place(
            "90210",
            "Beverly Hills",
            "California",
            "US",
            34.09,
            -118.41
        ))
    );
}

#[test]
fn nothing_within_30_km_finds_nothing() {
    let (_dir, index) = index();
    assert_eq!(index.nearest(0.0, 0.0), None);
    // Springfield, Illinois is about 55 km west.
    assert_eq!(index.nearest(39.8, -89.0), None);
    // About 27 km north of 62702 (39.82, -89.64) is still near enough.
    assert_eq!(
        index.nearest(40.065, -89.64).map(|p| p.code),
        Some("62702".into())
    );
    assert!(distance_km((40.095, -89.64), (39.82, -89.64)) > NEAREST_KM);
    assert_eq!(index.nearest(40.095, -89.64), None);
}

#[test]
fn a_point_near_a_cell_edge_finds_a_place_in_the_neighboring_cell() {
    let (_dir, index) = index();
    // Canning Town (51.51, 0.01) is in the cell north east of this point.
    assert_ne!(grid_key(51.49, -0.01), grid_key(51.51, 0.01));
    assert_eq!(
        index.nearest(51.49, -0.01).map(|p| p.name),
        Some("Canning Town".into())
    );
    // Across the antimeridian: Taveuni is at 179.95 east.
    assert_ne!(grid_key(-16.8, -179.98), grid_key(-16.8, 179.95));
    assert_eq!(index.nearest(-16.8, -179.98).map(|p| p.code), Some("0000".into()));
}

#[test]
fn coordinates_off_the_globe_find_nothing() {
    let (_dir, index) = index();
    assert_eq!(index.nearest(91.0, 0.0), None);
    assert_eq!(index.nearest(0.0, -180.5), None);
    assert_eq!(index.nearest(f64::NAN, 0.0), None);
}

#[test]
fn the_cells_searched_cover_30_km_at_every_latitude() {
    // At the equator the cell and its eight neighbors.
    assert_eq!(cells_near(0.2, 0.2).len(), 9);
    // Far north a column is narrower than 30 km, so more columns are read.
    assert!(cells_near(70.2, 10.2).len() > 9);
    // At a pole, the whole row.
    assert!(cells_near(89.99, 0.0).len() >= 720);
    for (lat, lon) in [
        (0.0, 0.0),
        (45.49, 7.99),
        (64.9, -150.1),
        (-33.5, 151.0),
        (78.2, 15.6),
    ] {
        let cells = cells_near(lat, lon);
        // Points 29 km away in eight directions all fall in a cell that is read.
        for bearing in 0..8 {
            let angle = f64::from(bearing) * std::f64::consts::FRAC_PI_4;
            let dlat = 29.0 / 111.19 * angle.cos();
            let dlon = 29.0 / (111.19 * lat.to_radians().cos()) * angle.sin();
            let (plat, plon) = (lat + dlat, lon + dlon);
            assert!(
                distance_km((lat, lon), (plat, plon)) < 30.0,
                "{lat},{lon} {bearing}"
            );
            assert!(
                cells.contains(&grid_key(plat, plon)),
                "{lat},{lon} bearing {bearing}"
            );
        }
    }
}

// ---- The file ------------------------------------------------------------------------------

#[test]
fn a_missing_file_finds_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let index = PlaceIndex::new(dir.path().join("places.bin"));
    assert_eq!(index.by_code("US", "90210"), None);
    assert_eq!(index.by_name("US", "Springfield", None), None);
}

#[test]
fn a_file_that_is_not_a_places_file_finds_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("places.bin");
    std::fs::write(&path, b"US|90210\tBeverly Hills\n".repeat(10)).expect("written");
    assert_eq!(PlaceIndex::new(&path).by_code("US", "90210"), None);
}

#[test]
fn a_file_put_in_place_later_is_read_then() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let index = PlaceIndex::new(dir.path().join("places.bin"));
    assert_eq!(index.by_code("US", "90210"), None);
    fixture_file(dir.path());
    assert_eq!(index.by_code("US", "90210").map(|p| p.code), Some("90210".into()));
}

// ---- The converter -------------------------------------------------------------------------

#[test]
fn the_converter_counts_what_it_kept_and_left_out() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut rows = FIXTURE.to_vec();
    rows.push("US\t12345\tNowhere\tNew York\tNY\t\t\t\t\tnorth\t-73.9\t");
    rows.push("US\t12346\tFar Out\tNew York\tNY\t\t\t\t\t95.0\t-73.9\t");
    rows.push("US\t\tNo Code\tNew York\tNY\t\t\t\t\t40.0\t-73.9\t");
    rows.push("USA\t10001\tNew York\tNew York\tNY\t\t\t\t\t40.0\t-73.9\t");
    rows.push("US\t10001\tToo short");
    rows.push("");
    let summary = write_places(&dir.path().join("places.bin"), &rows);
    assert_eq!(summary.rows, FIXTURE.len() + 5);
    assert_eq!(summary.skipped, 5);
    // Every fixture code once (59-700 is listed twice).
    assert_eq!(summary.codes, FIXTURE.len() - 1);
    // Beverly Hills, Allston, Springfield (Illinois), Springfield (Massachusetts), London,
    // Chiyoda, Zürich, Rakowice, Golnice, Canning Town, Taveuni and Eastern Alberta.
    assert_eq!(summary.cities, 12);
    // Every row but 01103, at the same rounded coordinate as 01101.
    assert_eq!(summary.points, FIXTURE.len() - 1);
    let bytes = std::fs::read(dir.path().join("places.bin")).expect("read");
    assert_eq!(summary.bytes, bytes.len() as u64);
    assert_eq!(&bytes[..8], MAGIC);
    assert_eq!(&bytes[bytes.len() - 8..], MAGIC);
}

#[test]
fn the_converter_cuts_many_rows_into_blocks_that_are_all_found() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let rows: Vec<String> = (0..20_000)
        .map(|n| {
            format!(
                "DE\t{n:05}\tOrt {n}\tLand {}\t{:02}\t\t\t\t\t{}\t{}\t",
                n % 16,
                n % 16,
                47.0 + f64::from(n) / 10_000.0,
                6.0 + f64::from(n) / 5_000.0
            )
        })
        .collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let path = dir.path().join("places.bin");
    let summary = write_places(&path, &rows);
    assert_eq!((summary.codes, summary.cities), (20_000, 20_000));

    let index = PlaceIndex::new(&path);
    let blocks = std::fs::File::open(&path)
        .and_then(|mut file| index::read_blocks_for_tests(&mut file))
        .expect("a block list");
    assert!(blocks.0 > 10 && blocks.1 > 10 && blocks.2 > 1, "{blocks:?}");
    for n in [0, 1, 4_999, 10_000, 15_432, 19_999] {
        let code = format!("{n:05}");
        let found = index.by_code("DE", &code).unwrap_or_else(|| panic!("{code}"));
        assert_eq!(found.name, format!("Ort {n}"));
        assert_eq!(found.latitude, round2(47.0 + f64::from(n) / 10_000.0));
        let city = index
            .by_name("DE", &format!("ort {n}"), None)
            .unwrap_or_else(|| panic!("{n}"));
        assert_eq!(city.code, code);
    }
    assert_eq!(index.by_code("DE", "20000"), None);
    assert_eq!(index.by_code("DE", "00000 "), index.by_code("DE", "00000"));
}

#[test]
fn a_lookup_is_quick() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let rows: Vec<String> = (0..50_000)
        .map(|n| format!("FR\t{n:05}\tVille {n}\tRégion\t11\t\t\t\t\t48.85\t2.35\t"))
        .collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let path = dir.path().join("places.bin");
    write_places(&path, &rows);
    let index = PlaceIndex::new(&path);
    index.by_code("FR", "00001").expect("warm");
    let started = std::time::Instant::now();
    for n in (0..50_000).step_by(997) {
        index.by_code("FR", &format!("{n:05}")).expect("found");
    }
    let each = started.elapsed() / 51;
    assert!(each < std::time::Duration::from_millis(50), "{each:?} a lookup");
    let started = std::time::Instant::now();
    index.nearest(48.85, 2.35).expect("found");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(50),
        "{:?}",
        started.elapsed()
    );
}
