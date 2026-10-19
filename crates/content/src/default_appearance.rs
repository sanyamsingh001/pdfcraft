//! Default appearance content uses PDF lexical rules, including comments and escaped names.

use crate::parse;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DefaultAppearance {
    pub font: Option<Vec<u8>>,
    pub size: Option<f64>,
    /// Gray, RGB, or CMYK components of the last nonstroking colour operator.
    pub color: Option<Vec<f64>>,
}

/// Read a small default appearance through the authoritative content lexer. Refuse damaged
/// lexical input or oversized strings rather than selecting a font from incomplete content.
pub fn parse_default_appearance(bytes: &[u8]) -> Result<DefaultAppearance, &'static str> {
    if bytes.len() > 65_536 {
        return Err("default appearance exceeds 65536 bytes");
    }
    let parsed = parse(bytes);
    if parsed.skipped != 0 {
        return Err("default appearance contains unreadable PDF content");
    }
    let mut out = DefaultAppearance::default();
    for op in parsed.ops {
        match op.op.as_slice() {
            b"Tf" => {
                if let [font, size] = op.operands.as_slice()
                    && let (Some(font), Some(size)) = (font.as_name(), size.as_f64())
                    && size.is_finite()
                    && size >= 0.0
                {
                    out.font = Some(font.to_vec());
                    out.size = Some(size);
                }
            }
            b"g" | b"rg" | b"k" => {
                let count = match op.op.as_slice() {
                    b"g" => 1,
                    b"rg" => 3,
                    _ => 4,
                };
                if op.operands.len() == count
                    && let Some(color) = op.operands.iter().map(|o| o.as_f64().filter(|v| v.is_finite())).collect::<Option<Vec<_>>>()
                {
                    out.color = Some(color);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Keep ordinary aliases unchanged. Names with escaped or non-ASCII bytes use a safe
/// generated alias; callers retain the raw name for source resource lookup.
pub fn generated_font_alias(raw: &[u8]) -> String {
    if !raw.is_empty() && raw.iter().all(|b| (b'!'..=b'~').contains(b) && !b"()<>[]{}/%#".contains(b)) {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        "PCDA".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_appearance_uses_pdf_names_comments_and_finite_operands() {
        let da = parse_default_appearance(b"/Field#20#FFFont 13 Tf 0.1 0.2 0.3 rg % /Wrong 200 Tf 1 0 0 rg\r\n").unwrap();
        assert_eq!(da.font.as_deref(), Some(b"Field \xffFont".as_slice()));
        assert_eq!(da.size, Some(13.0));
        assert_eq!(da.color, Some(vec![0.1, 0.2, 0.3]));
        let da = parse_default_appearance(b"/A 8 Tf 0 g /B#23C 9 Tf 0 0 0 1 k").unwrap();
        assert_eq!(da.font.as_deref(), Some(b"B#C".as_slice()));
        assert_eq!(da.color, Some(vec![0.0, 0.0, 0.0, 1.0]));
        assert_eq!(generated_font_alias(b"Field \xffFont"), "PCDA");
        assert_eq!(generated_font_alias(b"Helv"), "Helv");
    }

    #[test]
    fn default_appearance_refuses_unreadable_or_oversized_content() {
        assert!(parse_default_appearance(b"(unfinished").is_err());
        assert!(parse_default_appearance(&vec![b' '; 65_537]).is_err());
        assert!(parse_default_appearance(&vec![b' '; 65_536]).is_ok());
    }
}
