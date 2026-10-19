//! Contributor-original, valid static-XFA forms; no font assets are required.
use super::*;

const NAME: &str = "form1[0].page1[0].name[0]";
const AGREE: &str = "form1[0].page1[0].agree[0]";

fn original(value: &str, read_only: bool) -> Arc<Vec<u8>> {
    let data = format!("<form1><page1><name>{value}</name><agree>1</agree><keep>untouched</keep></page1></form1>");
    let mut cos = pdfcraft_cos::Document::open(Arc::new(pdfcraft_xfa::fixtures::static_shell(&data))).unwrap();
    pdfcraft_forms::set_value(&mut cos, NAME, &FieldValue::Text("Old".into())).unwrap();
    if read_only {
        let field = pdfcraft_forms::fields(&cos).into_iter().find(|f| f.name == NAME).unwrap();
        cos.update_dict(field.obj, |d| d.set(b"Ff".to_vec(), pdfcraft_cos::Object::Int(1))).unwrap();
    }
    Arc::new(write_full(&cos, &SaveOptions::default()).unwrap())
}

fn dataset(cos: &pdfcraft_cos::Document, name: &str) -> String {
    let packets = pdfcraft_xfa::read_packets(cos).unwrap().unwrap();
    let data = pdfcraft_xfa::parse_datasets(&packets.xdp).unwrap();
    data.text_at(&pdfcraft_xfa::som_to_path(name)).unwrap().to_string()
}

fn assert_saved(s: &Session, id: DocId, expected: &str) -> Arc<Vec<u8>> {
    let bytes = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    assert_eq!(dataset(&cos, NAME), expected);
    assert_eq!(dataset(&cos, "form1[0].page1[0].keep[0]"), "untouched");
    bytes
}

#[test]
fn rejected_xfa_imports_survive_other_fields_full_edits_and_reopen() {
    for (value, locked) in [("Ω", false), ("Ada", true)] {
        let bytes = original(value, locked);
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("static-xfa.pdf", None, bytes.clone(), None).unwrap();
        let doc = s.get(id).unwrap();
        assert_eq!(doc.form.iter().find(|f| f.name == NAME).unwrap().value, ["Old"]);
        assert_eq!(doc.form.iter().find(|f| f.name == AGREE).unwrap().value, ["1"], "a different dataset value imported successfully");
        assert!(doc.info.warnings.iter().any(|w| w.contains(NAME) && w.contains("original dataset value is preserved")));
        assert!(!doc.dirty);
        s.apply(id, Edit::SetFieldValue { name: AGREE.into(), value: FieldValue::Check(false) }).unwrap();
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let saved = assert_saved(&s, id, value);
        assert!(saved.starts_with(&bytes));
        let again = s.open("reopened.pdf", None, saved, None).unwrap();
        s.apply(again, Edit::SetFieldValue { name: AGREE.into(), value: FieldValue::Check(true) }).unwrap();
        assert_saved(&s, again, value);
    }
}

#[test]
fn explicit_xfa_replacement_is_atomic_and_undo_restores_dataset_protection() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("static-xfa.pdf", None, original("Ω", false), None).unwrap();
    let before = s.save_bytes(id).unwrap();
    assert!(s.apply(id, Edit::SetFieldValue { name: NAME.into(), value: FieldValue::Text("Ж".into()) }).is_err());
    assert_eq!(*s.save_bytes(id).unwrap(), *before);
    s.apply(id, Edit::SetFieldValue { name: NAME.into(), value: FieldValue::Text("Grace".into()) }).unwrap();
    assert_saved(&s, id, "Grace");
    s.undo(id).unwrap();
    assert_saved(&s, id, "Ω");
    s.redo(id).unwrap();
    assert_saved(&s, id, "Grace");
    s.undo(id).unwrap();
    s.apply(id, Edit::SetFieldValue { name: AGREE.into(), value: FieldValue::Check(false) }).unwrap();
    let saved = assert_saved(&s, id, "Ω");
    let again = s.open("again.pdf", None, saved, None).unwrap();
    s.apply(
        again,
        Edit::Batch {
            label: "Explicit replacement".into(),
            edits: vec![Edit::SetFieldValue { name: NAME.into(), value: FieldValue::Text("Ada".into()) }],
        },
    )
    .unwrap();
    assert_saved(&s, again, "Ada");
}

#[test]
fn indirect_xfa_replacements_refuse_without_changing_values_or_history() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("static-xfa.pdf", None, original("Ω", false), None).unwrap();
    let before = s.save_bytes(id).unwrap();
    let generation = s.get(id).unwrap().generation;
    for edit in [
        Edit::ResetForm { names: Some(vec![NAME.into()]) },
        Edit::DeleteField { name: NAME.into() },
        Edit::ApplyScriptChanges {
            changes: vec![pdfcraft_forms::FieldChange { name: NAME.into(), value: Some(vec!["Script value".into()]), ..Default::default() }],
        },
    ] {
        let error = s.apply(id, edit).unwrap_err().to_string();
        assert!(error.contains("unapplied XFA data"), "{error}");
        assert_eq!(*s.save_bytes(id).unwrap(), *before);
        assert_eq!(s.get(id).unwrap().generation, generation);
        assert!(matches!(s.undo(id), Err(EditError::NothingToUndo)));
    }
}

#[test]
fn xfa_preservation_checks_dataset_writes_even_when_widget_values_stay_unchanged() {
    let mut cos = pdfcraft_cos::Document::open(original("Ω", false)).unwrap();
    let preserved = xfa_unapplied_values(&cos);
    assert!(preserved.iter().any(|p| p.field.name == NAME));
    let fields = xfa_field_data(&cos);
    pdfcraft_xfa::write_data_ops(
        &mut cos,
        &[pdfcraft_xfa::DataOp::Value { path: pdfcraft_xfa::som_to_path(NAME), text: "Changed before appearance failure".into() }],
        None,
    )
    .unwrap();
    assert_eq!(xfa_field_data(&cos), fields);
    assert!(preserve_xfa_values(&cos, &preserved).is_err());
}

#[test]
fn shared_xfa_binding_cannot_overwrite_another_protected_field() {
    let mut cos = pdfcraft_cos::Document::open(original("Ω", false)).unwrap();
    let alias = pdfcraft_forms::add_field(&mut cos, 0, [72.0, 600.0, 300.0, 620.0], &NewField::Text { multiline: false }, Some("Alias")).unwrap();
    let field = pdfcraft_forms::fields(&cos).into_iter().find(|f| f.name == alias).unwrap();
    cos.update_dict(field.obj, |d| d.set(b"PCSom".to_vec(), pdfcraft_cos::PdfString::text(NAME))).unwrap();
    let bytes = Arc::new(write_full(&cos, &SaveOptions::default()).unwrap());
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("shared-binding.pdf", None, bytes, None).unwrap();
    let before = s.save_bytes(id).unwrap();
    let error = s.apply(id, Edit::SetFieldValue { name: NAME.into(), value: FieldValue::Text("Grace".into()) }).unwrap_err().to_string();
    assert!(error.contains("unapplied XFA data"), "{error}");
    assert_eq!(*s.save_bytes(id).unwrap(), *before);
    assert_saved(&s, id, "Ω");
}
