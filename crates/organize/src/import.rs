//! Copying pages between documents: the basis of Combine Files, Insert Pages from File,
//! Extract Pages and Split.
//!
//! A page is deep-copied with everything it references (content streams, resources, fonts,
//! images, annotations), renumbered into the destination. Objects shared between the copied
//! pages (a font used on every page) are copied once. The copy never follows `/Parent` from a
//! page, so the source page tree does not come along. It rewires what points *back* into the
//! document:
//! - an annotation's `/P` points at its new page;
//! - link and GoTo destinations that target a copied page point at the copy. Links to pages that
//!   were not copied are dropped, so they don't dangle;
//! - form-field widgets keep their field hierarchy, and the top-level fields are registered in the
//!   destination `/AcroForm`;
//! - structure-tree links (`/StructParent(s)`) and article beads (`/B`) are removed, because the
//!   source structure tree is not copied;
//! - named destinations are resolved against the source's name tree and become explicit ones;
//! - layers (optional content groups) used by the pages are registered in the destination's
//!   `/OCProperties`, keeping their default on/off state.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{INHERITABLE, OrganizeError, pages_root, rebuild, walk};

/// Keys that must not be followed when copying: they point back up or across the source
/// document, and following them would drag in unrelated objects.
const SKIP_ON_PAGE: &[&[u8]] = &[b"Parent", b"B", b"StructParents", b"Thumb", b"PieceInfo"];
const SKIP_ON_ANNOT: &[&[u8]] = &[b"StructParent", b"P"];

fn is_annotation(d: &Dict) -> bool {
    d.name(b"Type") == Some(b"Annot") || (d.contains(b"Subtype") && d.contains(b"Rect") && !d.contains(b"Type"))
}

struct Copier<'a> {
    src: &'a Document,
    map: HashMap<ObjRef, ObjRef>,
    /// Source page → destination page, for rewriting `/P` and destinations.
    pages: HashMap<ObjRef, ObjRef>,
    /// Destination objects that are annotation dictionaries (fixed up after copying).
    annots: Vec<ObjRef>,
    /// Top-level form fields reached through widgets.
    fields: Vec<ObjRef>,
    /// Optional content groups copied: (source, destination).
    ocgs: Vec<(ObjRef, ObjRef)>,
}

impl Copier<'_> {
    /// Repeated pages own fresh annotations, including their reply/popup relationships.
    /// Keep immutable resources and shared field parents in the document-wide copy cache.
    /// Return local source references and the cache bindings to restore after a repeated copy.
    fn page_annotation_copies(
        &mut self,
        entries: &[Object],
        page: ObjRef,
        repeated: bool,
        owners: &HashMap<ObjRef, ObjRef>,
    ) -> HashMap<ObjRef, Option<ObjRef>> {
        let listed: HashSet<_> = entries.iter().filter_map(Object::as_ref).collect();
        let mut pending = Vec::new();
        for entry in entries {
            if let Some(r) = entry.as_ref() {
                pending.push(r);
            } else if let Some(d) = entry.as_dict().filter(|d| is_annotation(d)) {
                pending.extend([b"Popup".as_slice(), b"IRT", b"Parent"].iter().filter_map(|key| d.reference(key)));
            }
        }
        let mut seen = HashSet::new();
        let mut local = HashMap::new();
        while let Some(r) = pending.pop() {
            if !seen.insert(r) {
                continue;
            }
            let object = self.src.get(r);
            let Some(d) = object.as_dict().filter(|d| is_annotation(d)) else { continue };
            // /P is optional. Source Annots membership identifies cross-page targets even
            // without it; use /P only for annotations not listed on any source page.
            let owner = owners.get(&r).copied().or_else(|| d.reference(b"P"));
            if !listed.contains(&r) && owner.is_some_and(|owner| owner != page) {
                continue;
            }
            local.insert(r, if repeated { self.map.remove(&r) } else { None });
            pending.extend([b"Popup".as_slice(), b"IRT", b"Parent"].iter().filter_map(|key| d.reference(key)));
        }
        local
    }

    /// Copy an indirect object (once), returning its destination reference.
    fn copy_ref(&mut self, dst: &mut Document, r: ObjRef) -> ObjRef {
        if let Some(d) = self.map.get(&r) {
            return *d;
        }
        // Reserve the number first so cycles (field ↔ widget) terminate.
        let new = dst.add(Object::Null);
        self.map.insert(r, new);
        let obj = self.src.get(r);
        if obj.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"OCG")) {
            self.ocgs.push((r, new));
        }
        let copied = self.copy_value(dst, &obj, Some(new));
        dst.set(new, copied);
        new
    }

    fn copy_value(&mut self, dst: &mut Document, o: &Object, this: Option<ObjRef>) -> Object {
        match o {
            Object::Ref(r) => Object::Ref(self.copy_ref(dst, *r)),
            Object::Array(a) => Object::Array(a.iter().map(|x| self.copy_value(dst, x, None)).collect()),
            Object::Dict(d) => Object::Dict(self.copy_dict(dst, d, this)),
            Object::Stream(s) => {
                let mut s = s.clone();
                s.dict = self.copy_dict(dst, &s.dict, None);
                Object::Stream(s)
            }
            other => other.clone(),
        }
    }

    fn copy_dict(&mut self, dst: &mut Document, d: &Dict, this: Option<ObjRef>) -> Dict {
        let is_annot = is_annotation(d);
        let is_field = d.contains(b"FT") || (d.contains(b"T") && d.contains(b"Kids"));
        let mut out = Dict::new();
        for (k, v) in d.iter() {
            if is_annot && SKIP_ON_ANNOT.contains(&k.as_slice()) {
                continue;
            }
            // Destinations are rewritten afterwards (they may target pages copied later).
            if is_annot && (k.as_slice() == b"Dest" || k.as_slice() == b"A") {
                out.set(k.clone(), v.clone());
                continue;
            }
            out.set(k.clone(), self.copy_value(dst, v, None));
        }
        if let Some(this) = this {
            if is_annot {
                self.annots.push(this);
            }
            // A field (or widget) without a parent is a top-level field.
            if is_field && !d.contains(b"Parent") {
                self.fields.push(this);
            }
        }
        out
    }

    /// Map a source destination (explicit array or reference to one) to the destination
    /// document; `None` if it targets a page that was not copied.
    fn map_dest(&self, dest: &Object) -> Option<Object> {
        self.map_dest_depth(dest, 0)
    }

    fn map_dest_depth(&self, dest: &Object, depth: u8) -> Option<Object> {
        if depth > 4 {
            return None;
        }
        match dest {
            Object::Name(n) => self.map_dest_depth(&named_dest(self.src, n)?, depth + 1),
            Object::String(s) => self.map_dest_depth(&named_dest(self.src, &s.bytes)?, depth + 1),
            Object::Ref(_) => self.map_dest_depth(&self.src.resolve(dest), depth + 1),
            Object::Dict(d) => self.map_dest_depth(d.get(b"D")?, depth + 1),
            Object::Array(a) => {
                let page = a.first()?.as_ref()?;
                let new_page = *self.pages.get(&page)?;
                let mut a = a.clone();
                a[0] = Object::Ref(new_page);
                Some(Object::Array(a))
            }
            _ => None,
        }
    }

    /// Rewrite `/P`, `/Dest` and GoTo actions on copied annotations.
    fn fix_annotations(&self, dst: &mut Document, annot_pages: &HashMap<ObjRef, ObjRef>) -> Result<(), OrganizeError> {
        for &a in &self.annots {
            let src_dict = dst.get(a).as_dict().cloned();
            let Some(d) = src_dict else { continue };
            let mut d = d;
            if let Some(p) = annot_pages.get(&a) {
                d.set(b"P".to_vec(), Object::Ref(*p));
            }
            if let Some(dest) = d.get(b"Dest").cloned() {
                let resolved = self.src.resolve(&dest);
                match self.map_dest(&resolved) {
                    Some(nd) => d.set(b"Dest".to_vec(), nd),
                    None => {
                        d.remove(b"Dest");
                    }
                }
            }
            if let Some(action) = d.get(b"A").cloned() {
                let action = self.src.resolve(&action);
                if let Some(ad) = action.as_dict() {
                    if ad.name(b"S") == Some(b"GoTo") {
                        let target = ad.get(b"D").map(|x| self.src.resolve(x));
                        match target.and_then(|t| self.map_dest(&t)) {
                            Some(nd) => {
                                let mut na = Dict::new();
                                na.set(b"S".to_vec(), Object::name("GoTo"));
                                na.set(b"D".to_vec(), nd);
                                d.set(b"A".to_vec(), Object::Dict(na));
                            }
                            None => {
                                d.remove(b"A");
                            }
                        }
                    } else {
                        // Other actions (URI, Launch, JavaScript…) carry no page references.
                        let mut c = Copier {
                            src: self.src,
                            map: self.map.clone(),
                            pages: HashMap::new(),
                            annots: Vec::new(),
                            fields: Vec::new(),
                            ocgs: Vec::new(),
                        };
                        let copied = c.copy_value(dst, &Object::Dict(ad.clone()), None);
                        d.set(b"A".to_vec(), copied);
                    }
                }
            }
            // A repeated widget shares its logical field, but needs its own Kids entry.
            // The parent was copied once and may still list only the first page's widget.
            if d.name(b"Subtype") == Some(b"Widget")
                && let Some(parent) = d.reference(b"Parent")
                && let Some(mut kids) =
                    dst.get(parent).as_dict().and_then(|p| p.get(b"Kids")).map(|k| dst.resolve(k)).and_then(|k| k.as_array().cloned())
                && !kids.contains(&Object::Ref(a))
            {
                kids.push(Object::Ref(a));
                dst.update_dict(parent, |p| p.set(b"Kids".to_vec(), Object::Array(kids)))?;
            }
            dst.set(a, Object::Dict(d));
        }
        Ok(())
    }
}

/// Copy `o` from `src` into `dst`, reusing the copies listed in `map` (source → destination), so
/// an object the copied pages already brought along (an ICC profile) is not stored twice.
pub(crate) fn copy_object(dst: &mut Document, src: &Document, o: &Object, map: HashMap<ObjRef, ObjRef>) -> Object {
    let mut c = Copier { src, map, pages: HashMap::new(), annots: Vec::new(), fields: Vec::new(), ocgs: Vec::new() };
    c.copy_value(dst, o, None)
}

/// Copy pages `src_pages` (0-based, in the given order) of `src` into `dst`, inserting them at
/// position `at`. Returns the new page references in order.
///
/// Resources identical to ones already in `dst` (the same fonts, images, colour profiles) are
/// shared rather than stored twice (see `dedupe`).
pub fn import_pages(dst: &mut Document, src: &Document, src_pages: &[usize], at: usize) -> Result<Vec<ObjRef>, OrganizeError> {
    let first_new = dst.object_numbers().last().map_or(1, |n| n + 1);
    let pages = import_pages_mapped(dst, src, src_pages, at)?.pages;
    let created: Vec<ObjRef> = dst.object_numbers().into_iter().filter(|n| *n >= first_new).map(|n| ObjRef::new(n, dst.generation(n))).collect();
    crate::dedupe::dedupe_resources(dst, &created, true);
    Ok(pages)
}

// Field-only entries are inherited by widgets after splitting a merged terminal field.
const FIELD_KEYS: &[&[u8]] =
    &[b"FT", b"T", b"TU", b"TM", b"Ff", b"V", b"DV", b"Kids", b"DA", b"Q", b"DS", b"RV", b"Opt", b"MaxLen", b"TI", b"I", b"Lock", b"SV"];
// Annotation entries remain on the widget. Unknown entries are retained on both halves.
const WIDGET_KEYS: &[&[u8]] = &[
    b"Type",
    b"Subtype",
    b"Rect",
    b"P",
    b"AP",
    b"AS",
    b"MK",
    b"F",
    b"BS",
    b"Border",
    b"H",
    b"StructParent",
    b"Contents",
    b"NM",
    b"M",
    b"C",
    b"CA",
    b"OC",
    b"A",
];
const FIELD_EVENTS: &[&[u8]] = &[b"K", b"F", b"V", b"C"];
const WIDGET_EVENTS: &[&[u8]] = &[b"E", b"X", b"D", b"U", b"Fo", b"Bl", b"PO", b"PC", b"PV", b"PI"];

/// Only repeated pages need merged field/widgets split. Retain the existing field reference
/// and hierarchy in a private source clone, so the ordinary widget copier shares one field.
fn split_repeated_widgets(src: &Document, source: &[(ObjRef, Dict)], pages: &[usize]) -> Result<Option<Document>, OrganizeError> {
    let mut seen_pages = HashSet::new();
    let mut repeated = HashSet::new();
    let mut split = HashMap::new();
    let mut out = None;
    for &index in pages {
        if seen_pages.insert(index) || !repeated.insert(index) {
            continue;
        }
        // Page indices were validated against source before calling this helper.
        let page = source[index].0;
        let entries =
            src.get(page).as_dict().and_then(|d| d.get(b"Annots")).map(|a| src.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        let mut changed = false;
        let mut copied = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(r) = entry.as_ref() else {
                copied.push(entry);
                continue;
            };
            if let Some(&kid) = split.get(&r) {
                copied.push(Object::Ref(kid));
                changed = true;
                continue;
            }
            let object = src.get(r);
            let Some(d) = object.as_dict().filter(|d| {
                d.name(b"Subtype") == Some(b"Widget")
                    && d.get(b"Kids").is_none_or(|k| src.resolve(k).as_array().is_some_and(|kids| kids.is_empty()))
                    && (d.contains(b"T") || (d.contains(b"FT") && !d.contains(b"Parent")))
            }) else {
                copied.push(entry);
                continue;
            };
            let staged = out.get_or_insert_with(|| src.clone());
            let mut field = d.clone();
            let mut widget = d.clone();
            for key in FIELD_KEYS {
                widget.remove(key);
            }
            for key in WIDGET_KEYS {
                field.remove(key);
            }
            // AA may mix widget mouse/focus events with field validation/format events.
            // Each known trigger stays on its owning half; unknown trigger keys are retained.
            if let Some(aa) = d.get(b"AA").map(|a| src.resolve(a)).and_then(|a| a.as_dict().cloned()) {
                for (half, removed) in [(&mut field, WIDGET_EVENTS), (&mut widget, FIELD_EVENTS)] {
                    let mut actions = aa.clone();
                    for key in removed {
                        actions.remove(key);
                    }
                    if actions.is_empty() {
                        half.remove(b"AA");
                    } else {
                        half.set(b"AA".to_vec(), Object::Dict(actions));
                    }
                }
            }
            widget.set(b"Parent".to_vec(), Object::Ref(r));
            let kid = staged.add(Object::Dict(widget));
            field.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(kid)]));
            staged.set(r, Object::Dict(field));
            split.insert(r, kid);
            copied.push(Object::Ref(kid));
            changed = true;
        }
        if changed && let Some(staged) = out.as_mut() {
            staged.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(copied)))?;
        }
    }
    Ok(out)
}

/// A shared field may also own widgets on pages that were not selected. Keep only the
/// selected widgets and their ancestors in field Kids arrays before the generic copier can
/// follow them. Other field entries (including unknown metadata) stay untouched.
fn prune_unselected_field_kids(src: &Document, source: &[(ObjRef, Dict)], pages: &[usize]) -> Result<Option<Document>, OrganizeError> {
    let mut retained = HashSet::new();
    let mut ancestors = HashSet::new();
    for &index in pages {
        let Some((page, _)) = source.get(index) else { return Err(OrganizeError::NoSuchPage(index)) };
        let entries =
            src.get(*page).as_dict().and_then(|d| d.get(b"Annots")).map(|a| src.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for entry in entries {
            let object = src.resolve(&entry);
            let Some(widget) = object.as_dict().filter(|d| d.name(b"Subtype") == Some(b"Widget")) else { continue };
            if let Some(r) = entry.as_ref() {
                retained.insert(r);
            }
            let mut parent = widget.reference(b"Parent");
            while let Some(r) = parent {
                retained.insert(r);
                if !ancestors.insert(r) {
                    break;
                }
                parent = src.get(r).as_dict().and_then(|d| d.reference(b"Parent"));
            }
        }
    }
    let mut out = None;
    for parent in ancestors {
        let Some(kids) = src.get(parent).as_dict().and_then(|d| d.get(b"Kids")).map(|k| src.resolve(k)).and_then(|k| k.as_array().cloned()) else {
            continue;
        };
        // Valid field children are indirect. Keep any lenient direct entries rather than
        // dropping them just because they have no reference in the selected inventory.
        let selected: Vec<_> = kids.iter().filter(|kid| kid.as_ref().is_none_or(|r| retained.contains(&r))).cloned().collect();
        if selected.len() != kids.len() {
            let staged = out.get_or_insert_with(|| src.clone());
            staged.update_dict(parent, |d| d.set(b"Kids".to_vec(), Object::Array(selected)))?;
        }
    }
    Ok(out)
}

/// What `import_pages_mapped` copied.
struct Imported {
    /// The new pages, in order.
    pages: Vec<ObjRef>,
    /// Source page → destination page.
    page_map: HashMap<ObjRef, ObjRef>,
    /// Every source object copied → its copy.
    objects: HashMap<ObjRef, ObjRef>,
}

/// `import_pages`, also returning what was copied from where.
fn import_pages_mapped(dst: &mut Document, src: &Document, src_pages: &[usize], at: usize) -> Result<Imported, OrganizeError> {
    let source = walk(src)?;
    if let Some(bad) = src_pages.iter().find(|i| **i >= source.len()) {
        return Err(OrganizeError::NoSuchPage(*bad));
    }
    let normalized = split_repeated_widgets(src, &source, src_pages)?;
    let src = normalized.as_ref().unwrap_or(src);
    let selected_fields = prune_unselected_field_kids(src, &source, src_pages)?;
    let src = selected_fields.as_ref().unwrap_or(src);
    let mut annotation_owners = HashMap::new();
    for (page, _) in &source {
        if let Some(entries) = src.get(*page).as_dict().and_then(|d| d.get(b"Annots")).map(|a| src.resolve(a)).and_then(|a| a.as_array().cloned()) {
            for r in entries.iter().filter_map(Object::as_ref) {
                annotation_owners.entry(r).or_insert(*page);
            }
        }
    }
    let mut existing = walk(dst)?;
    let root = pages_root(dst)?;
    let mut copier = Copier { src, map: HashMap::new(), pages: HashMap::new(), annots: Vec::new(), fields: Vec::new(), ocgs: Vec::new() };
    // Allocate every destination page first, so destinations between copied pages resolve.
    let targets: Vec<(ObjRef, ObjRef, Dict)> = src_pages
        .iter()
        .map(|&i| {
            let (page, inherited) = &source[i];
            let new = match copier.pages.get(page) {
                Some(n) => *n, // the same page requested twice gets a second copy below
                None => dst.add(Object::Null),
            };
            copier.pages.insert(*page, new);
            (*page, new, inherited.clone())
        })
        .collect();
    let mut annot_pages = HashMap::new();
    let mut new_pages = Vec::new();
    let mut done: HashMap<ObjRef, ObjRef> = HashMap::new();
    for (page, new, inherited) in targets {
        // A page listed twice: make a second, independent page object sharing resources.
        let new = if done.contains_key(&page) { dst.add(Object::Null) } else { new };
        let src_dict = src.get(page).as_dict().cloned().unwrap_or_default();
        let mut d = Dict::new();
        for (k, v) in src_dict.iter() {
            if SKIP_ON_PAGE.contains(&k.as_slice()) {
                continue;
            }
            if k.as_slice() == b"Annots" {
                let list = src.resolve(v).as_array().cloned().unwrap_or_default();
                let repeated = done.contains_key(&page);
                let local = copier.page_annotation_copies(&list, page, repeated, &annotation_owners);
                let mut out = Vec::new();
                for a in list {
                    // Direct (inline) annotation dictionaries become indirect objects, so every
                    // copied annotation can be fixed up the same way.
                    let r = match a {
                        Object::Ref(r) => copier.copy_ref(dst, r),
                        Object::Dict(_) => {
                            let r = dst.add(Object::Null);
                            let c = copier.copy_value(dst, &a, Some(r));
                            dst.set(r, c);
                            r
                        }
                        _ => continue,
                    };
                    annot_pages.insert(r, new);
                    out.push(Object::Ref(r));
                }
                // Related local annotations (e.g. a Popup) may be reached through a dictionary
                // rather than the array. Give them the same owning page too.
                for (r, previous) in local {
                    if let Some(&copied) = copier.map.get(&r) {
                        annot_pages.insert(copied, new);
                    }
                    // Cross-page references continue targeting the first copy, as page
                    // destinations do; the repeated page already retains its fresh refs.
                    if repeated {
                        if let Some(previous) = previous {
                            copier.map.insert(r, previous);
                        } else {
                            copier.map.remove(&r);
                        }
                    }
                }
                d.set(b"Annots".to_vec(), Object::Array(out));
                continue;
            }
            d.set(k.clone(), copier.copy_value(dst, v, None));
        }
        for k in INHERITABLE {
            if !d.contains(k)
                && let Some(v) = inherited.get(k)
            {
                let v = copier.copy_value(dst, v, None);
                d.set(k.to_vec(), v);
            }
        }
        d.set(b"Type".to_vec(), Object::name("Page"));
        d.set(b"Parent".to_vec(), Object::Ref(root));
        dst.set(new, Object::Dict(d));
        done.insert(page, new);
        new_pages.push(new);
    }
    copier.fix_annotations(dst, &annot_pages)?;
    register_fields(dst, &copier.fields)?;
    register_layers(dst, src, &copier.ocgs)?;
    let at = at.min(existing.len());
    let inserted: Vec<(ObjRef, Dict)> = new_pages.iter().map(|r| (*r, Dict::new())).collect();
    existing.splice(at..at, inserted);
    rebuild(dst, &existing)?;
    Ok(Imported { pages: new_pages, page_map: copier.pages, objects: copier.map })
}

/// Look up a named destination in the source (`/Dests` dictionary or `/Names /Dests` tree).
fn named_dest(src: &Document, name: &[u8]) -> Option<Object> {
    let catalog = src.get(src.root()?).as_dict().cloned()?;
    if let Some(dests) = catalog.get(b"Dests").map(|d| src.resolve(d))
        && let Some(v) = dests.as_dict().and_then(|d| d.get(name))
    {
        return Some(src.resolve(v).as_ref().clone());
    }
    let names = src.resolve(catalog.get(b"Names")?);
    let tree = names.as_dict()?.get(b"Dests")?.clone();
    name_tree_lookup(src, &tree, name, 0)
}

fn name_tree_lookup(src: &Document, node: &Object, key: &[u8], depth: u8) -> Option<Object> {
    if depth > 32 {
        return None;
    }
    let node = src.resolve(node);
    let d = node.as_dict()?;
    if let Some(names) = d.get(b"Names").map(|n| src.resolve(n)).and_then(|n| n.as_array().cloned()) {
        for pair in names.chunks(2) {
            if let [k, v] = pair
                && src.resolve(k).as_string().is_some_and(|s| s.bytes == key)
            {
                return Some(src.resolve(v).as_ref().clone());
            }
        }
    }
    let kids = d.get(b"Kids").map(|k| src.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    kids.iter().find_map(|k| name_tree_lookup(src, k, key, depth + 1))
}

/// All (key, value) pairs of a name tree, in order.
fn name_tree_entries(src: &Document, node: &Object, depth: u8, out: &mut Vec<(Vec<u8>, Object)>) {
    if depth > 32 {
        return;
    }
    let node = src.resolve(node);
    let Some(d) = node.as_dict() else { return };
    if let Some(names) = d.get(b"Names").map(|n| src.resolve(n)).and_then(|n| n.as_array().cloned()) {
        for pair in names.chunks(2) {
            if let [k, v] = pair
                && let Some(k) = src.resolve(k).as_string()
            {
                out.push((k.bytes.clone(), v.clone()));
            }
        }
    }
    for k in d.get(b"Kids").map(|k| src.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default() {
        name_tree_entries(src, &k, depth + 1, out);
    }
}

/// Register copied optional content groups in the destination's `/OCProperties`, keeping each
/// layer's default visibility from the source.
fn register_layers(dst: &mut Document, src: &Document, ocgs: &[(ObjRef, ObjRef)]) -> Result<(), OrganizeError> {
    if ocgs.is_empty() {
        return Ok(());
    }
    let src_off: Vec<Object> = src
        .root()
        .and_then(|r| src.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"OCProperties").map(|o| src.resolve(o)))
        .and_then(|o| o.as_dict().and_then(|d| d.get(b"D")).map(|d| src.resolve(d)))
        .and_then(|d| d.as_dict().and_then(|d| d.get(b"OFF")).map(|o| src.resolve(o)))
        .and_then(|o| o.as_array().cloned())
        .unwrap_or_default();
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let catalog = dst.get(root).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?;
    let (props_ref, mut props) = match catalog.get(b"OCProperties") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let list = |d: &Dict, k: &[u8], doc: &Document| d.get(k).map(|v| doc.resolve(v)).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut all = list(&props, b"OCGs", dst);
    let mut config = props.get(b"D").map(|d| dst.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default();
    let (mut on, mut off, mut order) = (list(&config, b"ON", dst), list(&config, b"OFF", dst), list(&config, b"Order", dst));
    for (from, to) in ocgs {
        let r = Object::Ref(*to);
        if all.contains(&r) {
            continue;
        }
        all.push(r.clone());
        order.push(r.clone());
        if src_off.contains(&Object::Ref(*from)) { off.push(r) } else { on.push(r) }
    }
    props.set(b"OCGs".to_vec(), Object::Array(all));
    config.set(b"ON".to_vec(), Object::Array(on));
    config.set(b"OFF".to_vec(), Object::Array(off));
    config.set(b"Order".to_vec(), Object::Array(order));
    props.set(b"D".to_vec(), Object::Dict(config));
    match props_ref {
        Some(r) => dst.set(r, Object::Dict(props)),
        None => dst.update_dict(root, |c| c.set(b"OCProperties".to_vec(), Object::Dict(props)))?,
    }
    Ok(())
}

/// Add top-level fields to the destination's `/AcroForm /Fields` (creating the form if needed).
fn register_fields(dst: &mut Document, fields: &[ObjRef]) -> Result<(), OrganizeError> {
    if fields.is_empty() {
        return Ok(());
    }
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let catalog = dst.get(root).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?;
    let (form_ref, mut form) = match catalog.get(b"AcroForm") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let mut list = form.get(b"Fields").map(|f| dst.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    for f in fields {
        if !list.contains(&Object::Ref(*f)) {
            list.push(Object::Ref(*f));
        }
    }
    form.set(b"Fields".to_vec(), Object::Array(list));
    match form_ref {
        Some(r) => dst.set(r, Object::Dict(form)),
        None => {
            let r = dst.add(form);
            dst.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Ref(r)))?;
        }
    }
    Ok(())
}

/// A new document containing copies of `pages` from `src` (Extract Pages / Split).
/// Document information (title, author…) is carried over, and so is the print standard the
/// source declares (PDF/X: its output intents and identification, #263).
pub fn extract_pages(src: &Document, pages: &[usize]) -> Result<Document, OrganizeError> {
    let mut out = Document::new_empty();
    // One source: nothing to deduplicate.
    let imported = import_pages_mapped(&mut out, src, pages, 0)?;
    for key in crate::INFO_KEYS {
        if let Some(v) = crate::info(src, key) {
            crate::set_info(&mut out, key, &v)?;
        }
    }
    // After the title: the XMP packet repeats it.
    crate::pdfx::carry(&mut out, src, &crate::pdfx::PrintStandard::of(src), imported.objects)?;
    // The pages share their source's resource dictionary, so the part would otherwise carry every
    // XObject the source lists — images and all (#204). Keep only what these pages draw.
    crate::prune::prune_unused_xobjects(&mut out)?;
    Ok(out)
}

/// How to split a document.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitBy {
    /// Every `n` pages.
    PageCount(usize),
    /// Before each of these 0-based page indices (e.g. `[3, 7]` → 0–2, 3–6, 7–end).
    Before(Vec<usize>),
}

/// The page ranges (0-based, end-exclusive) a split produces.
pub fn split_ranges(page_count: usize, by: &SplitBy) -> Vec<std::ops::Range<usize>> {
    let mut cuts: Vec<usize> = match by {
        SplitBy::PageCount(n) => (1..).map(|i| i * (*n).max(1)).take_while(|c| *c < page_count).collect(),
        SplitBy::Before(v) => v.iter().copied().filter(|c| *c > 0 && *c < page_count).collect(),
    };
    cuts.sort_unstable();
    cuts.dedup();
    let mut ranges = Vec::new();
    let mut start = 0;
    for c in cuts.into_iter().chain(std::iter::once(page_count)) {
        if c > start {
            ranges.push(start..c);
            start = c;
        }
    }
    ranges
}

/// Split a document into several new documents.
pub fn split(src: &Document, by: &SplitBy) -> Result<Vec<Document>, OrganizeError> {
    let n = crate::page_count(src)?;
    split_ranges(n, by).into_iter().map(|r| extract_pages(src, &r.collect::<Vec<_>>())).collect()
}

/// Combine whole documents, in order, into a new document. Each source gets a top-level
/// bookmark (its `title`) pointing to its first page, like Acrobat's Combine Files.
/// The source's own bookmarks are nested (collapsed) under its entry, and its document-level
/// attachments are carried over. When every source declares the same print standard (PDF/X
/// with the same output intent), so does the result.
pub fn combine(sources: &[(&str, &Document)]) -> Result<Document, OrganizeError> {
    let all: Vec<(&str, &Document, Option<&[usize]>)> = sources.iter().map(|(t, d)| (*t, *d, None)).collect();
    combine_selected(&all)
}

/// Combine Files with chosen pages: each source contributes `pages` (0-based, in that order;
/// `None` for all of them). Bookmarks that point at pages left out lose their destination.
pub fn combine_selected(sources: &[(&str, &Document, Option<&[usize]>)]) -> Result<Document, OrganizeError> {
    let mut out = Document::new_empty();
    let mut marks = Vec::new();
    let mut attachments = Vec::new();
    // The first source's print standard, and what its pages brought along; it carries over
    // only when every source declares the same one.
    let mut standard: Option<(crate::pdfx::PrintStandard, &Document, HashMap<ObjRef, ObjRef>)> = None;
    let mut agreed = true;
    for (title, src, chosen) in sources {
        let n = crate::page_count(src)?;
        let pages: Vec<usize> = match chosen {
            Some(p) => {
                if let Some(bad) = p.iter().find(|i| **i >= n) {
                    return Err(OrganizeError::NoSuchPage(*bad));
                }
                p.to_vec()
            }
            None => (0..n).collect(),
        };
        let at = crate::page_count(&out)?;
        let imported = import_pages_mapped(&mut out, src, &pages, at)?;
        if let Some(first) = imported.pages.first() {
            marks.push((title.to_string(), *first, *src, imported.page_map));
        }
        collect_attachments(&mut out, src, &mut attachments);
        let declared = crate::pdfx::PrintStandard::of(src);
        match &standard {
            None => standard = Some((declared, *src, imported.objects)),
            Some((first, _, _)) => agreed &= first.same_as(&declared),
        }
    }
    if let Some((declared, src, objects)) = standard.filter(|_| agreed) {
        crate::pdfx::carry(&mut out, src, &declared, objects)?;
    }
    // Sources often share fonts, images and profiles (or are the same file): store them once.
    let all: Vec<ObjRef> = out.object_numbers().into_iter().map(|n| ObjRef::new(n, out.generation(n))).collect();
    crate::dedupe::dedupe_resources(&mut out, &all, false);
    add_outline(&mut out, &marks)?;
    set_attachments(&mut out, attachments)?;
    Ok(out)
}

/// Copy a source's document-level attachments (`/Names /EmbeddedFiles`) into `dst`.
fn collect_attachments(dst: &mut Document, src: &Document, out: &mut Vec<(Vec<u8>, Object)>) {
    let Some(tree) = src
        .root()
        .and_then(|r| src.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"Names").map(|n| src.resolve(n)))
        .and_then(|n| n.as_dict().and_then(|d| d.get(b"EmbeddedFiles").cloned()))
    else {
        return;
    };
    let mut entries = Vec::new();
    name_tree_entries(src, &tree, 0, &mut entries);
    let mut copier = Copier { src, map: HashMap::new(), pages: HashMap::new(), annots: Vec::new(), fields: Vec::new(), ocgs: Vec::new() };
    for (k, v) in entries {
        let v = copier.copy_value(dst, &v, None);
        out.push((k, v));
    }
}

/// Write attachments as a flat, sorted `/EmbeddedFiles` name tree; duplicate names get " (2)"…
fn set_attachments(dst: &mut Document, mut entries: Vec<(Vec<u8>, Object)>) -> Result<(), OrganizeError> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut seen = std::collections::HashSet::new();
    for (k, _) in entries.iter_mut() {
        let base = k.clone();
        let mut n = 2;
        while !seen.insert(k.clone()) {
            *k = [base.as_slice(), format!(" ({n})").as_bytes()].concat();
            n += 1;
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut names = Vec::new();
    for (k, v) in entries {
        names.push(Object::String(pdfcraft_cos::PdfString::literal(k)));
        names.push(v);
    }
    let mut tree = Dict::new();
    tree.set(b"Names".to_vec(), Object::Array(names));
    let tree = dst.add(tree);
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    dst.update_dict(root, |c| {
        let mut n = match c.get(b"Names") {
            Some(Object::Dict(d)) => d.clone(),
            _ => Dict::new(),
        };
        n.set(b"EmbeddedFiles".to_vec(), Object::Ref(tree));
        c.set(b"Names".to_vec(), Object::Dict(n));
    })?;
    Ok(())
}

type Mark<'a> = (String, ObjRef, &'a Document, HashMap<ObjRef, ObjRef>);

/// Replace the outline with one bookmark per source (title → its first page), each holding a
/// collapsed copy of that source's own bookmarks with destinations mapped to the copied pages.
fn add_outline(doc: &mut Document, marks: &[Mark<'_>]) -> Result<(), OrganizeError> {
    if marks.is_empty() {
        return Ok(());
    }
    let outlines = doc.add(Object::Null);
    let items: Vec<ObjRef> = marks.iter().map(|_| doc.add(Object::Null)).collect();
    let (Some(&first_item), Some(&last_item)) = (items.first(), items.last()) else { return Ok(()) };
    for (i, ((title, page, src, page_map), r)) in marks.iter().zip(&items).enumerate() {
        let mut d = Dict::new();
        d.set(b"Title".to_vec(), Object::String(pdfcraft_cos::PdfString::text(title)));
        d.set(b"Parent".to_vec(), Object::Ref(outlines));
        d.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(*page), Object::name("Fit")]));
        if i > 0 {
            d.set(b"Prev".to_vec(), Object::Ref(items[i - 1]));
        }
        if let Some(next) = items.get(i + 1) {
            d.set(b"Next".to_vec(), Object::Ref(*next));
        }
        let first = src
            .root()
            .and_then(|r| src.get(r).as_dict().cloned())
            .and_then(|c| c.get(b"Outlines").map(|o| src.resolve(o)))
            .and_then(|o| o.as_dict().and_then(|d| d.reference(b"First")));
        let mapper = Copier { src, map: HashMap::new(), pages: page_map.clone(), annots: Vec::new(), fields: Vec::new(), ocgs: Vec::new() };
        let mut budget = 10_000usize;
        if let Some((f, l, n)) = copy_outline_level(doc, &mapper, first, *r, 0, &mut budget) {
            d.set(b"First".to_vec(), Object::Ref(f));
            d.set(b"Last".to_vec(), Object::Ref(l));
            d.set(b"Count".to_vec(), Object::Int(-(n as i64))); // collapsed
        }
        doc.set(*r, Object::Dict(d));
    }
    let mut o = Dict::new();
    o.set(b"Type".to_vec(), Object::name("Outlines"));
    o.set(b"First".to_vec(), Object::Ref(first_item));
    o.set(b"Last".to_vec(), Object::Ref(last_item));
    o.set(b"Count".to_vec(), Object::Int(items.len() as i64));
    doc.set(outlines, Object::Dict(o));
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    doc.update_dict(root, |c| {
        c.set(b"Outlines".to_vec(), Object::Ref(outlines));
        c.set(b"PageMode".to_vec(), Object::name("UseOutlines"));
    })?;
    Ok(())
}

/// Copy one level of a source outline (siblings from `first`) under `parent`, recursing into
/// children. Returns (first, last, visible count). Cycle- and size-safe.
fn copy_outline_level(
    dst: &mut Document,
    m: &Copier<'_>,
    first: Option<ObjRef>,
    parent: ObjRef,
    depth: u8,
    budget: &mut usize,
) -> Option<(ObjRef, ObjRef, usize)> {
    if depth > 32 {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut made: Vec<ObjRef> = Vec::new();
    let mut total = 0;
    let mut cur = first;
    while let Some(r) = cur {
        if !seen.insert(r) || *budget == 0 {
            break;
        }
        *budget -= 1;
        let Some(item) = m.src.get(r).as_dict().cloned() else { break };
        cur = item.reference(b"Next");
        let new = dst.add(Object::Null);
        let mut d = Dict::new();
        if let Some(t) = item.get(b"Title").map(|t| m.src.resolve(t)) {
            d.set(b"Title".to_vec(), t.as_ref().clone());
        }
        for k in [&b"C"[..], b"F"] {
            if let Some(v) = item.get(k) {
                d.set(k.to_vec(), m.src.resolve(v).as_ref().clone());
            }
        }
        let target = item.get(b"Dest").cloned().or_else(|| {
            let a = m.src.resolve(item.get(b"A")?);
            let a = a.as_dict()?;
            (a.name(b"S") == Some(b"GoTo")).then(|| a.get(b"D").cloned()).flatten()
        });
        if let Some(dest) = target.and_then(|t| m.map_dest(&t)) {
            d.set(b"Dest".to_vec(), dest);
        }
        d.set(b"Parent".to_vec(), Object::Ref(parent));
        if let Some(prev) = made.last() {
            d.set(b"Prev".to_vec(), Object::Ref(*prev));
            dst.update_dict(*prev, |p| p.set(b"Next".to_vec(), Object::Ref(new))).ok()?;
        }
        if let Some((f, l, n)) = copy_outline_level(dst, m, item.reference(b"First"), new, depth + 1, budget) {
            d.set(b"First".to_vec(), Object::Ref(f));
            d.set(b"Last".to_vec(), Object::Ref(l));
            // Keep the source's open/closed state.
            let open = item.int(b"Count").is_some_and(|c| c > 0);
            d.set(b"Count".to_vec(), Object::Int(if open { n as i64 } else { -(n as i64) }));
        }
        dst.set(new, Object::Dict(d));
        made.push(new);
        total += 1;
    }
    Some((*made.first()?, *made.last()?, total))
}

/// Page `page` of `src` as a form XObject in `dst` (its content and resources, no annotations),
/// for backgrounds and watermarks taken from a PDF. The form's `/BBox` is the page's crop box
/// and its `/Matrix` undoes the page rotation, so it draws upright as displayed. Returns the
/// form and its displayed size in points.
pub fn page_as_form(dst: &mut Document, src: &Document, page: usize) -> Result<(ObjRef, (f64, f64)), OrganizeError> {
    use pdfcraft_cos::Stream;
    let all = walk(src)?;
    // `walk` gives the inheritable attributes (resources, boxes, rotation); the content is
    // the page's own.
    let (pr, d) = all.get(page).cloned().ok_or(OrganizeError::NoSuchPage(page))?;
    let own = src.get(pr).as_dict().cloned().unwrap_or_default();
    let rect = |k: &[u8]| -> Option<[f64; 4]> {
        let a = src.resolve(d.get(k)?);
        let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| src.resolve(x).as_f64()).collect();
        (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
    };
    let bbox = rect(b"CropBox").or_else(|| rect(b"MediaBox")).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    let rotate = d.get(b"Rotate").and_then(|r| src.resolve(r).as_int()).unwrap_or(0).rem_euclid(360);
    // Content: the page's streams, decoded and joined.
    let mut content = Vec::new();
    if let Some(c) = own.get(b"Contents") {
        let list = match &*src.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        };
        for o in list {
            if let Object::Stream(s) = &*src.resolve(&o) {
                content.extend(s.decoded().map_err(|e| OrganizeError::Invalid(format!("page {} content: {e}", page + 1)))?);
                content.push(b'\n');
            }
        }
    }
    let mut copier = Copier { src, map: HashMap::new(), pages: HashMap::new(), annots: Vec::new(), fields: Vec::new(), ocgs: Vec::new() };
    let resources = d.get(b"Resources").map(|r| copier.copy_value(dst, r, None)).unwrap_or(Object::Dict(Dict::new()));
    register_layers(dst, src, &copier.ocgs)?;
    let mut fd = Dict::new();
    fd.set(b"Type".to_vec(), Object::name("XObject"));
    fd.set(b"Subtype".to_vec(), Object::name("Form"));
    fd.set(b"BBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    // Map the crop box to (0, 0)–(w, h) as displayed (turning a rotated page upright).
    let m: [f64; 6] = match rotate {
        90 => [0.0, -1.0, 1.0, 0.0, -bbox[1], bbox[2]],
        180 => [-1.0, 0.0, 0.0, -1.0, bbox[2], bbox[3]],
        270 => [0.0, 1.0, -1.0, 0.0, bbox[3], -bbox[0]],
        _ => [1.0, 0.0, 0.0, 1.0, -bbox[0], -bbox[1]],
    };
    fd.set(b"Matrix".to_vec(), Object::Array(m.iter().map(|v| Object::Real(*v)).collect()));
    fd.set(b"Resources".to_vec(), resources);
    let r = dst.add(Object::Stream(Stream::flate(fd, &content)));
    let size = if rotate == 90 || rotate == 270 { (h, w) } else { (w, h) };
    Ok((r, size))
}

#[cfg(test)]
mod duplicate_annotations;
