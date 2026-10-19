//! Just enough CSV for spreadsheets saved from Excel: quoted fields,
//! doubled quotes, line breaks inside cells, the UTF-8 byte-order mark, and
//! `;` as the separator (Excel uses it in regions where `,` is the decimal
//! mark).

/// Rows of fields. Blank rows are dropped.
pub fn parse(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = detect_delimiter(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                '\r' => {}
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c if c == delimiter => row.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

/// `;` if the header line has more of them than commas.
fn detect_delimiter(text: &str) -> char {
    let header = text.lines().next().unwrap_or("");
    if header.matches(';').count() > header.matches(',').count() { ';' } else { ',' }
}

/// One CSV line (with `\r\n`, as Excel writes it).
pub fn row(fields: &[String]) -> String {
    let escaped: Vec<String> = fields
        .iter()
        .map(|f| {
            if f.contains([',', '"', '\n', '\r', ';']) {
                format!("\"{}\"", f.replace('"', "\"\""))
            } else {
                f.clone()
            }
        })
        .collect();
    escaped.join(",") + "\r\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|r| r.iter().map(|f| f.to_string()).collect()).collect()
    }

    #[test]
    fn reads_excel_csv() {
        let text = "\u{feff}file,said,wanted,tags\r\n\
            001.wav,\"um, hi\",Hi.,\"names, filler\"\r\n\
            002.wav,\"she said \"\"no\"\"\",\"line one\nline two\",\r\n\
            ,,,\r\n";
        assert_eq!(
            parse(text),
            strings(&[
                &["file", "said", "wanted", "tags"],
                &["001.wav", "um, hi", "Hi.", "names, filler"],
                &["002.wav", "she said \"no\"", "line one\nline two", ""],
            ])
        );
    }

    #[test]
    fn reads_semicolon_csv_without_final_newline() {
        assert_eq!(parse("file;said\n003.wav;one, two"), strings(&[&["file", "said"], &["003.wav", "one, two"]]));
    }

    #[test]
    fn written_rows_read_back() {
        let fields: Vec<String> = ["a,b", "say \"hi\"", "two\nlines", "plain"].map(String::from).to_vec();
        assert_eq!(parse(&row(&fields)), vec![fields]);
    }
}
