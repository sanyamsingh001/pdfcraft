//! Reserve generated names within one page resource category.

use pdfcraft_cos::{Dict, Object};

/// Keep every existing entry. Reuse only an exactly identical object/value; otherwise
/// find a free alias. The caller inserts the returned name before reserving another.
pub(crate) fn reserve(entries: &Dict, preferred: &str, value: &Object) -> String {
    let mut name = preferred.to_string();
    loop {
        match entries.get(name.as_bytes()) {
            None => return name,
            Some(existing) if existing == value => return name,
            Some(_) => name.push('_'),
        }
    }
}

#[cfg(test)]
mod tests;
