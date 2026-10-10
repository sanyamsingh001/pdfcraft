//! Contributor-original finite page geometry and physical UserUnit controls.

use std::sync::Arc;

use hayro::hayro_interpret::{InterpreterSettings, TransformExt};
use hayro::hayro_syntax::Pdf;

use crate::{PageRenderer, RenderConfig, RenderRequest, RequestKind, inspect};

fn fixture(rotation: i32, local_unit: Option<&str>) -> Arc<Vec<u8>> {
    let content = "0 0 1 rg 20 30 10 10 re f 0 0 0 rg BT /F 6 Tf 1 0 0 1 14 66 Tm (Unit) Tj ET";
    let unit = local_unit.map(|value| format!("/UserUnit {value}")).unwrap_or_default();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        // UserUnit is not inheritable: this parent value must never affect a leaf.
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /UserUnit 9 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 60 100] /CropBox [10 20 50 80] /Rotate {rotation} {unit} /Resources << /Font << /F 5 0 R >> >> /Contents 4 0 R >>"
        ),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    Arc::new(bytes)
}

// Independent clockwise rotation of a raw crop-relative point, then y-down display.
fn expected_view(rotation: i32, unit: f32, x: f32, y: f32) -> [f32; 2] {
    let (x, y) = (x - 10.0, y - 20.0);
    let (x, y) = match rotation {
        90 => (y, x),
        180 => (40.0 - x, y),
        270 => (60.0 - y, 40.0 - x),
        _ => (x, 60.0 - y),
    };
    [x * unit, y * unit]
}

fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.002, "{a} != {b}");
}

#[test]
fn physical_geometry_raster_and_text_scale_once_with_local_user_units() {
    for rotation in [0, 90, 180, 270] {
        let default_pdf = Pdf::new(fixture(rotation, None)).unwrap();
        let default_text = crate::text::extract_page(&default_pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(default_text.plain_text(), "Unit");
        for (entry, unit) in [(None, 1.0), (Some("1"), 1.0), (Some("2"), 2.0), (Some("0.5"), 0.5)] {
            let bytes = fixture(rotation, entry);
            let info = inspect(bytes.clone(), None).unwrap();
            let page_info = &info.pages[0];
            let expected_size = if rotation % 180 == 0 { (40.0 * unit, 60.0 * unit) } else { (60.0 * unit, 40.0 * unit) };
            assert_eq!((page_info.width, page_info.height), expected_size);
            assert_eq!(page_info.crop, [10.0, 20.0, 50.0, 80.0]);
            assert_eq!(page_info.user_unit, unit);
            let pdf = Pdf::new(bytes.clone()).unwrap();
            let pages = pdf.pages();
            let page = &pages[0];
            assert_eq!(page.user_unit(), unit);
            assert_eq!(page.render_dimensions(), expected_size);
            for [x, y] in [[10.0, 20.0], [50.0, 20.0], [10.0, 80.0], [50.0, 80.0], [25.0, 35.0]] {
                let expected = expected_view(rotation, unit, x, y);
                let transformed = page.initial_transform(true).to_kurbo() * kurbo::Point::new(x as f64, y as f64);
                close(transformed.x as f32, expected[0]);
                close(transformed.y as f32, expected[1]);
                let view = page_info.user_to_view(x, y);
                close(view[0], expected[0]);
                close(view[1], expected[1]);
                let raw = page_info.view_to_user(view[0], view[1]);
                close(raw[0], x);
                close(raw[1], y);
            }
            let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
            let raster = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            assert!(raster.error.is_none(), "{:?}", raster.error);
            assert_eq!((raster.width, raster.height), (expected_size.0 as u32, expected_size.1 as u32));
            let center = expected_view(rotation, unit, 25.0, 35.0);
            let offset = ((center[1] as u32 * raster.width + center[0] as u32) * 4) as usize;
            assert_eq!(&raster.rgba[offset..offset + 4], &[0, 0, 255, 255]);
            let text = crate::text::extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
            assert_eq!(text.plain_text(), "Unit");
            assert_eq!(text.find("Unit"), vec![0..text.glyphs.len()]);
            assert_eq!(text.glyphs.len(), default_text.glyphs.len());
            for (glyph, original) in text.glyphs.iter().zip(&default_text.glyphs) {
                assert_eq!(glyph.text, original.text);
                for (actual, original) in glyph.rect.into_iter().zip(original.rect) {
                    close(actual, original * unit);
                }
            }
        }
    }
}

#[test]
fn malformed_local_user_units_keep_the_default_physical_size() {
    for entry in ["0", "-1", "75001", "/WrongType", "null"] {
        let bytes = fixture(0, Some(entry));
        let info = inspect(bytes.clone(), None).unwrap();
        assert_eq!((info.pages[0].width, info.pages[0].height), (40.0, 60.0));
        assert_eq!(info.pages[0].user_unit, 1.0);
        let pdf = Pdf::new(bytes).unwrap();
        assert_eq!(pdf.pages()[0].user_unit(), 1.0);
    }
}
