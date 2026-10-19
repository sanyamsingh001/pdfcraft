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
/// lexical input, oversized strings, or styling that a generated appearance cannot retain.
/// Explicit initial text-state values are harmless; balanced graphics saves restore the
/// selected font and nonstroking colour.
pub fn parse_default_appearance(bytes: &[u8]) -> Result<DefaultAppearance, &'static str> {
    if bytes.len() > 65_536 {
        return Err("default appearance exceeds 65536 bytes");
    }
    let parsed = parse(bytes);
    if parsed.skipped != 0 {
        return Err("default appearance contains unreadable PDF content");
    }
    let mut out = DefaultAppearance::default();
    let mut stack = Vec::new();
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
                } else {
                    return Err("default appearance has invalid font operands");
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
                } else {
                    return Err("default appearance has invalid colour operands");
                }
            }
            // Generated appearances use these initial text-state values. Other values
            // affect spacing, geometry or painting and cannot be silently discarded.
            b"Tc" | b"Tw" | b"Ts" | b"TL" | b"Tz" | b"Tr" => {
                let default = if op.is("Tz") { 100.0 } else { 0.0 };
                if !matches!(op.operands.as_slice(), [value] if value.as_f64() == Some(default)) {
                    return Err("default appearance text styling cannot be preserved");
                }
            }
            b"q" if op.operands.is_empty() => {
                if stack.len() >= 256 {
                    return Err("default appearance graphics state exceeds 256 levels");
                }
                stack.push(out.clone());
            }
            b"Q" if op.operands.is_empty() => {
                out = stack.pop().ok_or("default appearance has an unmatched graphics state restore")?;
            }
            _ => return Err("default appearance operator cannot be preserved"),
        }
    }
    if !stack.is_empty() {
        return Err("default appearance has an unmatched graphics state save");
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

    #[test]
    fn default_appearance_accepts_initial_text_state_and_restores_saved_style() {
        let da = parse_default_appearance(b"/Field#20Font 13 Tf 0.2 g 0 Tc -0 Tw 100 Tz 0 Tr 0 Ts 0 TL % 5 Tc\n q /Wrong 7 Tf 1 0 0 rg Q").unwrap();
        assert_eq!(da.font.as_deref(), Some(b"Field Font".as_slice()));
        assert_eq!(da.size, Some(13.0));
        assert_eq!(da.color, Some(vec![0.2]));
        assert!(parse_default_appearance(&b"q ".repeat(256).into_iter().chain(b"Q ".repeat(256)).collect::<Vec<_>>()).is_ok());
        assert!(parse_default_appearance(&b"q ".repeat(257).into_iter().chain(b"Q ".repeat(257)).collect::<Vec<_>>()).is_err());
    }

    #[test]
    fn default_appearance_refuses_nondefault_and_unhandled_styling() {
        for state in
            ["1 Tc", "2 Tw", "80 Tz", "1 Tr", "2 Ts", "14 TL", "/State gs", "1 0 0 1 2 3 cm", "q", "Q", "0 1 Tc", "/Bad Tf", "0 1 g", "0 g Unknown"]
        {
            let da = format!("/F1 12 Tf 0 g {state}");
            assert!(parse_default_appearance(da.as_bytes()).is_err(), "{state}");
        }
    }
}
