//! GeoNames' postal code dump (`allCountries.txt`, tab-separated) into a places file.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};

use flate2::Compression;
use flate2::write::DeflateEncoder;

use super::{
    CITY_SECTION, GRID_SECTION, MAGIC, POSTAL_SECTION, code_key, country_key, grid_key, name_key, round2,
};

/// About how many bytes of lines go in one block before it is deflated.
const BLOCK_BYTES: usize = 16 * 1024;

/// The longest field kept, in characters; longer ones are cut.
const FIELD_CHARS: usize = 120;

/// What a conversion read and wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConvertSummary {
    /// Rows read.
    pub rows: usize,
    /// Rows left out: too few columns, no country, no code or no coordinates.
    pub skipped: usize,
    /// Postal codes written (one per country and code).
    pub codes: usize,
    /// Cities written (one per country, city and state).
    pub cities: usize,
    /// Points written to the grid (one per coordinate, rounded to 2 decimals).
    pub points: usize,
    /// Bytes written.
    pub bytes: u64,
}

/// One city: every postal code row with its country, name and state, averaged.
struct City {
    code: String,
    name: String,
    state: String,
    state_code: String,
    latitude: f64,
    longitude: f64,
    count: u32,
}

/// Reads the GeoNames rows (country, postal code, place, admin name 1, admin code 1, admin
/// name 2, admin code 2, admin name 3, admin code 3, latitude, longitude, accuracy) and writes
/// the places file. A postal code listed more than once keeps its first row; a city's
/// coordinates are the mean of its postal codes'; of the postal codes at one rounded coordinate,
/// the first is the grid's point there.
pub fn convert(input: impl BufRead, output: impl Write) -> io::Result<ConvertSummary> {
    let mut summary = ConvertSummary::default();
    let mut codes: Vec<(String, String)> = Vec::new();
    let mut cities: HashMap<String, City> = HashMap::new();
    let mut points: Vec<(String, String)> = Vec::new();
    let mut pointed: HashSet<(i64, i64)> = HashSet::new();

    for line in input.lines() {
        let line = line?;
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        summary.rows += 1;
        let fields: Vec<&str> = line.split('\t').collect();
        let parsed = (fields.len() >= 11)
            .then(|| {
                let country = country_key(fields[0])?;
                let code = clean(fields[1]);
                let key = code_key(&code);
                let latitude = fields[9].trim().parse::<f64>().ok()?;
                let longitude = fields[10].trim().parse::<f64>().ok()?;
                let in_range = (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude);
                (!key.is_empty() && in_range).then_some((country, code, key, latitude, longitude))
            })
            .flatten();
        let Some((country, code, key, latitude, longitude)) = parsed else {
            summary.skipped += 1;
            continue;
        };
        let name = clean(fields[2]);
        let state = clean(fields[3]);
        let state_code = clean(fields[4]);

        let written = if code == key { "" } else { code.as_str() };
        let (lat, lon) = (round2(latitude), round2(longitude));
        codes.push((
            format!("{country}|{key}"),
            format!("\t{written}\t{name}\t{state}\t{lat}\t{lon}"),
        ));
        // Hundredths of a degree, as whole numbers, so equal rounded coordinates are equal.
        if pointed.insert(((lat * 100.0).round() as i64, (lon * 100.0).round() as i64)) {
            points.push((
                grid_key(lat, lon),
                format!("\t{country}\t{code}\t{name}\t{state}\t{lat}\t{lon}"),
            ));
        }

        let city_name = name_key(&name);
        if city_name.is_empty() {
            continue;
        }
        let state_name = name_key(if state.is_empty() { &state_code } else { &state });
        let city = cities
            .entry(format!("{country}|{city_name}|{state_name}"))
            .or_insert_with(|| City {
                code: code.clone(),
                name: name.clone(),
                state: state.clone(),
                state_code: state_code.clone(),
                latitude: 0.0,
                longitude: 0.0,
                count: 0,
            });
        city.latitude += latitude;
        city.longitude += longitude;
        city.count += 1;
    }

    // Stable, so the first row of a code listed twice is the one kept.
    codes.sort_by(|a, b| a.0.cmp(&b.0));
    codes.dedup_by(|later, earlier| later.0 == earlier.0);
    summary.codes = codes.len();

    let mut city_lines: Vec<(String, String)> = cities
        .into_iter()
        .map(|(key, city)| {
            let count = f64::from(city.count);
            let line = format!(
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                city.code,
                city.name,
                city.state,
                city.state_code,
                round2(city.latitude / count),
                round2(city.longitude / count),
                city.count
            );
            (key, line)
        })
        .collect();
    city_lines.sort_by(|a, b| a.0.cmp(&b.0));
    summary.cities = city_lines.len();

    // Stable: within a cell, points keep the order of their rows.
    points.sort_by(|a, b| a.0.cmp(&b.0));
    summary.points = points.len();

    let mut writer = BlockWriter::new(output);
    writer.raw(MAGIC)?;
    writer.section(POSTAL_SECTION, &codes)?;
    writer.section(CITY_SECTION, &city_lines)?;
    writer.section(GRID_SECTION, &points)?;
    summary.bytes = writer.finish()?;
    Ok(summary)
}

/// A field with no tabs or line breaks, trimmed and cut to [`FIELD_CHARS`].
fn clean(field: &str) -> String {
    field
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(FIELD_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Writes the blocks and then the block list.
struct BlockWriter<W: Write> {
    out: W,
    at: u64,
    list: String,
}

impl<W: Write> BlockWriter<W> {
    fn new(out: W) -> Self {
        BlockWriter {
            out,
            at: 0,
            list: String::new(),
        }
    }

    fn raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.at += bytes.len() as u64;
        Ok(())
    }

    /// The sorted lines of one section, `(key, rest of the line)`, cut into blocks.
    fn section(&mut self, section: &str, lines: &[(String, String)]) -> io::Result<()> {
        let mut block = String::new();
        let mut first_key: Option<&str> = None;
        for (key, rest) in lines {
            if first_key.is_none() {
                first_key = Some(key);
            }
            block.push_str(key);
            block.push_str(rest);
            block.push('\n');
            if block.len() >= BLOCK_BYTES {
                self.block(section, first_key.take().unwrap_or_default(), &block)?;
                block.clear();
            }
        }
        if let Some(first) = first_key {
            self.block(section, first, &block)?;
        }
        Ok(())
    }

    fn block(&mut self, section: &str, first_key: &str, lines: &str) -> io::Result<()> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(lines.as_bytes())?;
        let packed = encoder.finish()?;
        self.list.push_str(&format!(
            "{section}\t{first_key}\t{}\t{}\n",
            self.at,
            packed.len()
        ));
        self.raw(&packed)
    }

    /// The block list and the trailer; the bytes written in all.
    fn finish(mut self) -> io::Result<u64> {
        let list_at = self.at;
        let list = std::mem::take(&mut self.list);
        self.raw(list.as_bytes())?;
        self.raw(&list_at.to_le_bytes())?;
        self.raw(&(list.len() as u64).to_le_bytes())?;
        self.raw(MAGIC)?;
        self.out.flush()?;
        Ok(self.at)
    }
}
