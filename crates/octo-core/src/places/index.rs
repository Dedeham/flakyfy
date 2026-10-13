//! Looking a place up in a places file, a block at a time.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use flate2::read::DeflateDecoder;
use parking_lot::Mutex;
use tracing::warn;

use super::{
    CITY_SECTION, GRID_SECTION, MAGIC, NEAREST_KM, POSTAL_SECTION, Place, cells_near, coarser_codes,
    code_key, country_key, distance_km, name_key, round2,
};

/// The trailer: the block list's offset and length, then the magic.
const TRAILER_BYTES: u64 = 24;

/// The largest block list read, in bytes: far above what a full GeoNames dump makes.
const MAX_LIST_BYTES: u64 = 16 * 1024 * 1024;

/// The largest block read, packed or unpacked, in bytes.
const MAX_BLOCK_BYTES: u64 = 1024 * 1024;

/// One block of a section: its first key, and where its bytes are.
#[derive(Debug)]
struct Block {
    first_key: String,
    offset: u64,
    length: u64,
}

/// The block list of every section, read once.
#[derive(Debug, Default)]
struct Blocks {
    postal: Vec<Block>,
    cities: Vec<Block>,
    grid: Vec<Block>,
}

/// The places file and its block list, read the first time a lookup needs it. A file that is
/// not there (a dev run, a test) or cannot be read finds nothing, and says so in the log once.
pub struct PlaceIndex {
    path: PathBuf,
    blocks: Mutex<Option<Arc<Blocks>>>,
    warned: AtomicBool,
}

impl PlaceIndex {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        PlaceIndex {
            path: path.into(),
            blocks: Mutex::new(None),
            warned: AtomicBool::new(false),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The place with this postal code in `country` (ISO 3166-1 alpha-2, any case). The code
    /// is matched as a key (case, spaces and dashes ignored); a code the data holds only part of
    /// is found by its coarser code (see [`coarser_codes`]).
    pub fn by_code(&self, country: &str, code: &str) -> Option<Place> {
        let country = country_key(country)?;
        let key = code_key(code);
        if key.is_empty() {
            return None;
        }
        self.read(|reader, blocks| {
            for candidate in std::iter::once(key.clone()).chain(coarser_codes(&country, &key)) {
                let wanted = format!("{country}|{candidate}");
                let mut found = None;
                scan(reader, &blocks.postal, &wanted, |line_key, line| {
                    if line_key == wanted {
                        found = postal_place(&country, &candidate, line);
                    }
                    false
                })?;
                if found.is_some() {
                    return Ok(found);
                }
            }
            Ok(None)
        })
    }

    /// The city called `name` in `country`. With `state` (its name or its code, as the data
    /// writes them), only a city in that state; without, the one with the most postal codes,
    /// which is the largest of the cities with that name.
    pub fn by_name(&self, country: &str, name: &str, state: Option<&str>) -> Option<Place> {
        let country = country_key(country)?;
        let city = name_key(name);
        if city.is_empty() {
            return None;
        }
        let state = state.map(name_key).filter(|s| !s.is_empty());
        let prefix = format!("{country}|{city}|");
        self.read(|reader, blocks| {
            let mut best: Option<(u32, Place)> = None;
            scan(reader, &blocks.cities, &prefix, |line_key, line| {
                let Some(state_key) = line_key.strip_prefix(prefix.as_str()) else {
                    return false;
                };
                if let Some((count, place, state_code)) = city_place(&country, line) {
                    let wanted = state
                        .as_deref()
                        .is_none_or(|s| s == state_key || s == name_key(&state_code));
                    if wanted && best.as_ref().is_none_or(|(most, _)| count > *most) {
                        best = Some((count, place));
                    }
                }
                true
            })?;
            Ok(best.map(|(_, place)| place))
        })
    }

    /// The place nearest a coordinate (decimal degrees), at most [`NEAREST_KM`] away, or `None`.
    /// Only the grid cells that can hold such a place are read (see [`cells_near`]).
    pub fn nearest(&self, latitude: f64, longitude: f64) -> Option<Place> {
        let valid = (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude);
        if !valid {
            return None;
        }
        self.read(|reader, blocks| {
            let mut best: Option<(f64, Place)> = None;
            for cell in cells_near(latitude, longitude) {
                scan(reader, &blocks.grid, &cell, |line_key, line| {
                    if line_key != cell {
                        return false;
                    }
                    if let Some(place) = grid_place(line) {
                        let km = distance_km((latitude, longitude), (place.latitude, place.longitude));
                        if km <= NEAREST_KM && best.as_ref().is_none_or(|(nearest, _)| km < *nearest) {
                            best = Some((km, place));
                        }
                    }
                    true
                })?;
            }
            Ok(best.map(|(_, place)| place))
        })
    }

    /// Runs `lookup` over the open file and its block list; `None` (logged once) when the file
    /// is missing or unreadable.
    fn read(&self, lookup: impl FnOnce(&mut Reader, &Blocks) -> io::Result<Option<Place>>) -> Option<Place> {
        let outcome = File::open(&self.path).and_then(|mut file| {
            let blocks = self.blocks(&mut file)?;
            lookup(&mut Reader::new(&mut file), &blocks)
        });
        match outcome {
            Ok(place) => place,
            Err(e) => {
                if !self.warned.swap(true, Ordering::Relaxed) {
                    warn!(
                        "Place lookups find nothing: the place data at {} cannot be read ({e})",
                        self.path.display()
                    );
                }
                None
            }
        }
    }

    fn blocks(&self, file: &mut File) -> io::Result<Arc<Blocks>> {
        if let Some(blocks) = self.blocks.lock().as_ref() {
            return Ok(Arc::clone(blocks));
        }
        let blocks = Arc::new(read_blocks(file)?);
        *self.blocks.lock() = Some(Arc::clone(&blocks));
        Ok(blocks)
    }
}

fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// The block list, after checking both magics.
fn read_blocks(file: &mut File) -> io::Result<Blocks> {
    let size = file.seek(SeekFrom::End(0))?;
    if size < MAGIC.len() as u64 + TRAILER_BYTES {
        return Err(bad("not a places file (too short)"));
    }
    let mut head = [0u8; 8];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    let mut trailer = [0u8; TRAILER_BYTES as usize];
    file.seek(SeekFrom::Start(size - TRAILER_BYTES))?;
    file.read_exact(&mut trailer)?;
    if &head != MAGIC || &trailer[16..] != MAGIC {
        return Err(bad("not a places file"));
    }
    let word = |at: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&trailer[at..at + 8]);
        u64::from_le_bytes(bytes)
    };
    let (list_at, list_len) = (word(0), word(8));
    if list_len > MAX_LIST_BYTES || list_at.saturating_add(list_len) > size - TRAILER_BYTES {
        return Err(bad("the block list is out of the file"));
    }
    let mut list = String::new();
    file.seek(SeekFrom::Start(list_at))?;
    file.by_ref().take(list_len).read_to_string(&mut list)?;

    let mut blocks = Blocks::default();
    for line in list.lines() {
        let mut fields = line.split('\t');
        let (Some(section), Some(first_key), Some(offset), Some(length)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(bad("a block list line is short"));
        };
        let (Ok(offset), Ok(length)) = (offset.parse::<u64>(), length.parse::<u64>()) else {
            return Err(bad("a block list line has no offset"));
        };
        if length > MAX_BLOCK_BYTES || offset.saturating_add(length) > list_at {
            return Err(bad("a block is out of the file"));
        }
        let block = Block {
            first_key: first_key.to_string(),
            offset,
            length,
        };
        match section {
            POSTAL_SECTION => blocks.postal.push(block),
            CITY_SECTION => blocks.cities.push(block),
            GRID_SECTION => blocks.grid.push(block),
            _ => {}
        }
    }
    Ok(blocks)
}

/// How many blocks each section has: postal codes, cities, the grid.
#[cfg(test)]
pub(super) fn read_blocks_for_tests(file: &mut File) -> io::Result<(usize, usize, usize)> {
    let blocks = read_blocks(file)?;
    Ok((blocks.postal.len(), blocks.cities.len(), blocks.grid.len()))
}

/// The open places file, with the last block it unpacked, so the cells of one nearest lookup
/// that share a block unpack it once.
struct Reader<'a> {
    file: &'a mut File,
    last: Option<(u64, String)>,
}

impl<'a> Reader<'a> {
    fn new(file: &'a mut File) -> Self {
        Reader { file, last: None }
    }

    /// The lines of `block`, unpacked.
    fn text(&mut self, block: &Block) -> io::Result<&str> {
        if self
            .last
            .as_ref()
            .is_none_or(|(offset, _)| *offset != block.offset)
        {
            let mut packed = Vec::new();
            self.file.seek(SeekFrom::Start(block.offset))?;
            self.file.by_ref().take(block.length).read_to_end(&mut packed)?;
            let mut text = String::new();
            DeflateDecoder::new(packed.as_slice())
                .take(MAX_BLOCK_BYTES)
                .read_to_string(&mut text)?;
            self.last = Some((block.offset, text));
        }
        Ok(self.last.as_ref().map_or("", |(_, text)| text.as_str()))
    }
}

/// Visits the lines of a section whose key is `from` or after it, in order, while `visit`
/// returns true: `visit(key, rest of the line)`.
fn scan(
    reader: &mut Reader,
    blocks: &[Block],
    from: &str,
    mut visit: impl FnMut(&str, &str) -> bool,
) -> io::Result<()> {
    // The first block that can hold `from`: the last one whose first key is before it.
    let start = blocks
        .partition_point(|block| block.first_key.as_str() < from)
        .saturating_sub(1);
    for block in &blocks[start.min(blocks.len())..] {
        let text = reader.text(block)?;
        for line in text.lines() {
            let (key, rest) = line.split_once('\t').unwrap_or((line, ""));
            if key < from {
                continue;
            }
            if !visit(key, rest) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// A postal code line: written code (empty when the key), place, state, latitude, longitude.
fn postal_place(country: &str, key: &str, rest: &str) -> Option<Place> {
    let fields: Vec<&str> = rest.split('\t').collect();
    let [written, name, state, latitude, longitude, ..] = fields.as_slice() else {
        return None;
    };
    Some(Place {
        code: if written.is_empty() { key } else { *written }.to_string(),
        name: name.to_string(),
        state: state.to_string(),
        country: country.to_string(),
        latitude: latitude.parse().ok()?,
        longitude: longitude.parse().ok()?,
    })
}

/// A city line: a postal code, city, state, state code, latitude, longitude, postal codes.
/// The place with its count of postal codes and its state code.
fn city_place(country: &str, rest: &str) -> Option<(u32, Place, String)> {
    let fields: Vec<&str> = rest.split('\t').collect();
    let [code, name, state, state_code, latitude, longitude, count, ..] = fields.as_slice() else {
        return None;
    };
    let place = Place {
        code: code.to_string(),
        name: name.to_string(),
        state: state.to_string(),
        country: country.to_string(),
        latitude: latitude.parse().ok()?,
        longitude: longitude.parse().ok()?,
    };
    Some((count.parse().ok()?, place, state_code.to_string()))
}

/// A grid line: country, code as written, place, state, latitude, longitude.
fn grid_place(rest: &str) -> Option<Place> {
    let fields: Vec<&str> = rest.split('\t').collect();
    let [country, code, name, state, latitude, longitude, ..] = fields.as_slice() else {
        return None;
    };
    Some(Place {
        code: code.to_string(),
        name: name.to_string(),
        state: state.to_string(),
        country: country.to_string(),
        latitude: round2(latitude.parse().ok()?),
        longitude: round2(longitude.parse().ok()?),
    })
}
