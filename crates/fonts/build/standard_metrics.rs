//! Deterministic extraction from the already attributed hayro standard-font data.
//! The vendor sources and their licences remain authoritative; no duplicate data is tracked.
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

const FACES: [&str; 12] = [
    "COURIER",
    "COURIER_BOLD",
    "COURIER_OBLIQUE",
    "COURIER_BOLD_OBLIQUE",
    "HELVETICA",
    "HELVETICA_BOLD",
    "HELVETICA_OBLIQUE",
    "HELVETICA_BOLD_OBLIQUE",
    "TIMES_ROMAN",
    "TIMES_BOLD",
    "TIMES_ITALIC",
    "TIMES_BOLD_ITALIC",
];

fn read(dir: &Path, file: &str) -> Result<String, String> {
    let path = dir.join(file);
    println!("cargo::rerun-if-changed={}", path.display());
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

// This intentionally accepts only the reviewed vendor table format. A vendor update that
// changes its shape fails clearly instead of quietly generating incomplete font metrics.
fn table<'a>(source: &'a str, name: &str) -> Result<&'a str, String> {
    let marker = format!("static {name}:");
    let (_, tail) = source.split_once(marker.as_str()).ok_or_else(|| format!("missing vendor table {name}"))?;
    let (declaration, tail) = tail.split_once("phf_map! {").ok_or_else(|| format!("missing map for {name}"))?;
    if declaration.contains('\n') {
        return Err(format!("unexpected declaration for {name}"));
    }
    tail.split_once("\n};").map(|(body, _)| body).ok_or_else(|| format!("unterminated vendor table {name}"))
}

fn pairs(body: &str) -> Result<Vec<(&str, &str)>, String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.strip_suffix(',')
                .and_then(|line| line.split_once(" => "))
                .map(|(key, value)| (key.trim(), value.trim()))
                .ok_or_else(|| format!("unexpected vendor entry: {line}"))
        })
        .collect()
}

fn glyph(value: &str) -> Result<&str, String> {
    let name = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')).ok_or_else(|| format!("invalid glyph name {value}"))?;
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_')) {
        return Err(format!("unexpected glyph name {value}"));
    }
    Ok(name)
}

pub fn generate(dir: &Path) -> Result<String, String> {
    let metrics = read(dir, "metrics.rs")?;
    let mut tables = Vec::new();
    for face in FACES {
        let mut widths = BTreeMap::new();
        for (key, value) in pairs(table(&metrics, face)?)? {
            let name = glyph(key)?;
            let width = value.strip_suffix("_f32").and_then(|s| s.parse::<u16>().ok()).ok_or_else(|| format!("invalid advance {value}"))?;
            if widths.insert(name, width).is_some() {
                return Err(format!("duplicate glyph in {face}: {name}"));
            }
        }
        if widths.len() != 315 {
            return Err(format!("expected 315 glyphs in {face}, got {}", widths.len()));
        }
        tables.push(widths);
    }
    let first = tables.first().ok_or("no standard-face metrics")?;
    if tables.iter().any(|table| table.keys().ne(first.keys())) {
        return Err("standard faces have different glyph sets".into());
    }
    let mut output = String::from(
        "// Generated from attributed vendor/hayro-interpret standard-font data.\n// Copyright (c) The Hayro Authors; MIT licence retained in vendor/hayro-interpret/LICENSE-MIT.\nconst LATIN_ADVANCES: &[(&str, [u16; 12])] = &[\n",
    );
    for name in first.keys() {
        let widths: Vec<_> = tables.iter().filter_map(|table| table.get(name).copied()).collect();
        let _ = writeln!(output, "    ({name:?}, {widths:?}),");
    }
    output.push_str("];\n");
    for (file, name, count) in [("win_ansi.rs", "WIN_ANSI", 218), ("mac_roman.rs", "MAC_ROMAN", 208), ("standard.rs", "STANDARD", 149)] {
        let source = read(dir, file)?;
        let mut encoding = BTreeMap::new();
        for (key, value) in pairs(table(&source, name)?)? {
            let code = key.strip_suffix("_u8").and_then(|s| s.parse::<u8>().ok()).ok_or_else(|| format!("invalid encoding code {key}"))?;
            if encoding.insert(code, glyph(value)?).is_some() {
                return Err(format!("duplicate code in {name}: {code}"));
            }
        }
        if encoding.len() != count {
            return Err(format!("expected {count} entries in {name}, got {}", encoding.len()));
        }
        let _ = writeln!(output, "const {name}: [&str; 256] = [");
        for code in 0..=255u8 {
            // Same fallback as vendor win_ansi::get; the other base encodings have none.
            let fallback = if name == "WIN_ANSI" && code > 40 { "bullet" } else { "" };
            let value = encoding.get(&code).copied().unwrap_or(fallback);
            let _ = writeln!(output, "    {value:?},");
        }
        output.push_str("];\n");
    }
    Ok(output)
}
