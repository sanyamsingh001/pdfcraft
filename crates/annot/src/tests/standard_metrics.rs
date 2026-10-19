//! Contributor-original persisted appearance checks, independent of Metrics/TextFonts widths.
use super::*;

#[test]
fn automatically_sized_helvetica_runs_fit_using_independent_standard_advances() {
    for (count, border) in [(30, 0.0), (60, 0.0), (30, 2.0), (60, 2.0)] {
        let mut doc = fixture();
        let text = "O".repeat(count);
        let bounds = appearance::auto_text_rect(&doc, [100.0, 700.0], &text, 10.0, border, [40.0, 300.0]).unwrap();
        let mut annotation = new(0, Shape::TextBox { rect: bounds, font_size: 10.0 });
        annotation.contents = text.clone();
        annotation.style.width = border;
        let index = add_annotation(&mut doc, &annotation, &meta("exact-standard-fit")).unwrap();
        let doc = reopen(&doc);
        let d = &list(&doc, 0)[index];
        let ap = doc.resolve(d.get(b"AP").unwrap());
        let normal = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
        let Object::Stream(stream) = &*normal else { panic!("appearance stream") };
        let resources = doc.resolve(stream.dict.get(b"Resources").unwrap());
        let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
        let bytes = stream.decoded_strict().unwrap();
        let mut lexer = pdfcraft_cos::Lexer::new(&bytes, 0);
        let mut operands = Vec::new();
        let (mut x, mut size) = (0.0, 0.0);
        let (mut shown, mut lines) = (0, 0);
        loop {
            lexer.skip_ws();
            let Some(next) = bytes.get(lexer.pos) else { break };
            let start = lexer.pos;
            if matches!(next, b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.' | b'0'..=b'9') {
                operands.push(lexer.object().unwrap());
                assert!(lexer.pos > start, "operand parser must advance");
                continue;
            }
            let op = lexer.token();
            assert!(lexer.pos > start, "operator parser must advance");
            match op {
                b"Tm" => {
                    x = operands[4].as_f64().unwrap();
                    lines += 1;
                }
                b"Tf" => {
                    let selected = operands[0].as_name().unwrap();
                    let font = doc.resolve(fonts.as_dict().unwrap().get(selected).unwrap());
                    assert_eq!(font.as_dict().unwrap().name(b"BaseFont"), Some(b"Helvetica".as_slice()));
                    assert_eq!(font.as_dict().unwrap().name(b"Encoding"), Some(b"WinAnsiEncoding".as_slice()));
                    assert!(!font.as_dict().unwrap().contains(b"Widths"));
                    size = operands[1].as_f64().unwrap();
                    assert_eq!(size, 10.0);
                }
                b"Tj" => {
                    let bytes = &operands[0].as_string().unwrap().bytes;
                    assert!(bytes.iter().all(|byte| *byte == b'O'));
                    shown += bytes.len();
                    // Standard Helvetica O has 778 units/em; deliberately never consult Metrics.
                    let end = x + bytes.len() as f64 * size * 0.778;
                    assert!(x >= bounds[0] && end <= bounds[2] + 0.001, "persisted run clips: {x}..{end} vs {bounds:?}");
                    x = end;
                }
                _ => {}
            }
            operands.clear();
        }
        assert_eq!(shown, count, "all original text bytes remain in the saved appearance");
        assert_eq!(lines, if count == 30 { 1 } else { 2 });
        assert_eq!(appearance_text(&doc, d), text);
    }
}
