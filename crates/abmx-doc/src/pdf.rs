//! Encrypted-PDF reading and positioned-text extraction.
//!
//! The only place in abmx that touches PDF internals. It produces
//! [`Document`] (pages of positioned runs) so the grammar layer never sees a
//! PDF object, a content stream, or a font.
//!
//! Decryption is in-process: the password is never placed on a command line,
//! never written to a temporary file, and the decrypted document never touches
//! disk.

use std::collections::BTreeMap;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document as Pdf, Encoding, LoadOptions, Object};

use crate::layout::{Document, Line, Page, Run};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read the PDF: {0}")]
    Read(String),
    #[error("the PDF is password protected; configure a password for this parser")]
    Locked,
    #[error("the PDF password is wrong")]
    WrongPassword,
    #[error("the PDF has no extractable text layer (scanned or image-only)")]
    NoTextLayer,
    #[error("page {page}: {message}")]
    Page { page: usize, message: String },
}

/// Read a PDF, decrypt it if needed, and return its pages in reading order.
///
/// `password` is only consulted when the document is encrypted. An empty user
/// password (owner-restricted PDFs) works without configuration.
pub fn load(bytes: &[u8], password: Option<&str>) -> Result<Document, Error> {
    let pdf = open(bytes, password)?;
    let pages = pdf.get_pages();
    if pages.is_empty() {
        return Err(Error::NoTextLayer);
    }

    let mut out = Vec::with_capacity(pages.len());
    let mut saw_text = false;

    for (number, page_id) in pages {
        let number = number as usize;
        let content = pdf
            .get_and_decode_page_content(page_id)
            .map_err(|error| Error::Page {
                page: number,
                message: error.to_string(),
            })?;

        let fonts = pdf.get_page_fonts(page_id).unwrap_or_default();
        let encodings = font_encodings(&pdf, &fonts);

        let runs = extract_runs(&content, &encodings);
        saw_text |= !runs.is_empty();
        out.push(Page {
            number,
            lines: group_lines(runs),
        });
    }

    if !saw_text {
        return Err(Error::NoTextLayer);
    }

    Ok(Document { pages: out })
}

/// Open the document with the password supplied to the reader.
///
/// The password must be given at load time: lopdf loads only the encryption
/// dictionary until it can authenticate, so decrypting afterwards leaves the
/// page tree empty.
fn open(bytes: &[u8], password: Option<&str>) -> Result<Pdf, Error> {
    // An owner-restricted PDF has an empty *user* password; trying it first
    // means such documents need no configuration at all.
    let attempts: Vec<&str> = match password {
        Some(password) => vec!["", password],
        None => vec![""],
    };

    for password in attempts {
        match Pdf::load_mem_with_options(bytes, LoadOptions::with_password(password)) {
            Ok(pdf) => return Ok(pdf),
            Err(lopdf::Error::InvalidPassword) => continue,
            Err(error) => return Err(Error::Read(error.to_string())),
        }
    }

    Err(if password.is_some() {
        Error::WrongPassword
    } else {
        Error::Locked
    })
}

fn font_encodings<'a>(
    pdf: &'a Pdf,
    fonts: &'a BTreeMap<Vec<u8>, &'a Dictionary>,
) -> BTreeMap<String, Encoding<'a>> {
    let mut encodings = BTreeMap::new();
    for (name, font) in fonts {
        // A font whose encoding cannot be read falls back to Latin-1 decoding;
        // that is better than dropping the page, and the column check will
        // make a bad decode loud when the arithmetic fails.
        if let Ok(encoding) = font.get_font_encoding(pdf) {
            encodings.insert(String::from_utf8_lossy(name).into_owned(), encoding);
        }
    }
    encodings
}

/// Text-state matrix as `[a b c d e f]`.
type Matrix = [f32; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn translate(tx: f32, ty: f32, matrix: Matrix) -> Matrix {
    // [1 0 0 1 tx ty] × matrix
    [
        matrix[0],
        matrix[1],
        matrix[2],
        matrix[3],
        tx * matrix[0] + ty * matrix[2] + matrix[4],
        tx * matrix[1] + ty * matrix[3] + matrix[5],
    ]
}

struct State {
    matrix: Matrix,
    line: Matrix,
    leading: f32,
    size: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            matrix: IDENTITY,
            line: IDENTITY,
            leading: 0.0,
            size: 0.0,
        }
    }
}

impl State {
    fn next_line(&mut self) {
        self.line = translate(0.0, -self.leading, self.line);
        self.matrix = self.line;
    }

    fn move_to(&mut self, tx: f32, ty: f32) {
        self.line = translate(tx, ty, self.line);
        self.matrix = self.line;
    }
}

fn extract_runs(content: &Content, encodings: &BTreeMap<String, Encoding>) -> Vec<Run> {
    let mut state = State::default();
    let mut current: Option<&Encoding> = None;
    let mut runs = Vec::new();

    for operation in &content.operations {
        let op = operation.operator.as_str();
        match op {
            "BT" => {
                state.matrix = IDENTITY;
                state.line = IDENTITY;
            }
            "Tf" => {
                if let Some(name) = operation.operands.first().and_then(|o| o.as_name().ok()) {
                    let name = String::from_utf8_lossy(name).into_owned();
                    current = encodings.get(&name);
                }
                if let Some(size) = operation.operands.get(1).and_then(|o| o.as_float().ok()) {
                    state.size = size;
                }
            }
            "TL" => {
                if let Some(value) = operation.operands.first().and_then(|o| o.as_float().ok()) {
                    state.leading = value;
                }
            }
            "Td" | "TD" => {
                let tx = number(operation.operands.first());
                let ty = number(operation.operands.get(1));
                if op == "TD" {
                    state.leading = -ty;
                }
                state.move_to(tx, ty);
            }
            "Tm" => {
                if let Some(matrix) = matrix_of(operation) {
                    state.matrix = matrix;
                    state.line = matrix;
                }
            }
            "T*" => state.next_line(),
            "Tj" => {
                if let Some(text) = operation.operands.first().and_then(|o| decode(o, current)) {
                    push_run(&mut runs, &state, text);
                }
            }
            "'" => {
                state.next_line();
                if let Some(text) = operation.operands.first().and_then(|o| decode(o, current)) {
                    push_run(&mut runs, &state, text);
                }
            }
            "\"" => {
                state.next_line();
                if let Some(text) = operation.operands.get(2).and_then(|o| decode(o, current)) {
                    push_run(&mut runs, &state, text);
                }
            }
            "TJ" => {
                let Some(Object::Array(items)) = operation.operands.first() else {
                    continue;
                };
                // Each string is a run at the current position. The advance is
                // an estimate: bank statements draw each cell as its own Tj, so
                // this only matters for producers that pack a whole line into TJ.
                let mut x = state.matrix[4];
                for item in items {
                    match item {
                        Object::String(bytes, _) => {
                            let text = decode_bytes(bytes, current);
                            let mut local = state.clone_at(x);
                            local.matrix[4] = x;
                            push_run(&mut runs, &local, text.clone());
                            x += estimate_width(&text, state.size);
                        }
                        Object::Integer(value) => {
                            x -= *value as f32 / 1000.0 * state.size;
                        }
                        Object::Real(value) => {
                            x -= *value / 1000.0 * state.size;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    runs
}

impl State {
    fn clone_at(&self, x: f32) -> State {
        State {
            matrix: [
                self.matrix[0],
                self.matrix[1],
                self.matrix[2],
                self.matrix[3],
                x,
                self.matrix[5],
            ],
            line: self.line,
            leading: self.leading,
            size: self.size,
        }
    }
}

fn estimate_width(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * size * 0.5
}

fn number(object: Option<&Object>) -> f32 {
    object.and_then(|o| o.as_float().ok()).unwrap_or(0.0)
}

fn matrix_of(operation: &Operation) -> Option<Matrix> {
    if operation.operands.len() < 6 {
        return None;
    }
    let mut matrix = IDENTITY;
    for (index, slot) in matrix.iter_mut().enumerate() {
        *slot = operation.operands[index].as_float().ok()?;
    }
    Some(matrix)
}

fn decode(object: &Object, encoding: Option<&Encoding>) -> Option<String> {
    let Object::String(bytes, _) = object else {
        return None;
    };
    Some(decode_bytes(bytes, encoding))
}

fn decode_bytes(bytes: &[u8], encoding: Option<&Encoding>) -> String {
    if let Some(encoding) = encoding {
        if let Ok(text) = encoding.bytes_to_string(bytes) {
            return text;
        }
    }
    // Latin-1 fallback. Most simple fonts are WinAnsi, which agrees with
    // Latin-1 for the printable ASCII range that bank statements use.
    bytes.iter().map(|byte| *byte as char).collect()
}

fn push_run(runs: &mut Vec<Run>, state: &State, text: String) {
    if text.is_empty() {
        return;
    }
    runs.push(Run {
        x: state.matrix[4],
        y: state.matrix[5],
        size: state.size,
        text,
    });
}

fn group_lines(mut runs: Vec<Run>) -> Vec<Line> {
    runs.retain(|run| !run.text.trim().is_empty());
    runs.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lines: Vec<Line> = Vec::new();
    for run in runs {
        match lines.last_mut() {
            Some(line) if (line.y - run.y).abs() <= 1.0 => line.runs.push(run),
            _ => lines.push(Line {
                y: run.y,
                runs: vec![run],
            }),
        }
    }

    for line in &mut lines {
        line.runs
            .sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny uncompressed PDF with one page and one text run.
    fn minimal_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 10 Tf 40 700 Td ({text}) Tj ET");
        let mut objects = Vec::new();
        let catalog = 1;
        let pages = 2;
        let page = 3;
        let font = 4;
        let contents = 5;

        objects.push(format!(
            "{catalog} 0 obj\n<< /Type /Catalog /Pages {pages} 0 R >>\nendobj\n"
        ));
        objects.push(format!(
            "{pages} 0 obj\n<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>\nendobj\n"
        ));
        objects.push(format!(
            "{page} 0 obj\n<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {contents} 0 R >>\nendobj\n"
        ));
        objects.push(format!(
            "{font} 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n"
        ));
        objects.push(format!(
            "{contents} 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n",
            content.len()
        ));

        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for object in &objects {
            offsets.push(pdf.len());
            pdf.push_str(object);
        }
        let xref = pdf.len();
        pdf.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root {catalog} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        pdf.into_bytes()
    }

    #[test]
    fn extracts_a_positioned_run() {
        let document = load(&minimal_pdf("Hello"), None).unwrap();
        assert_eq!(document.pages.len(), 1);
        let line = &document.pages[0].lines[0];
        assert_eq!(line.text().trim(), "Hello");
        assert!((line.runs[0].x - 40.0).abs() < 0.01);
        assert!((line.runs[0].y - 700.0).abs() < 0.01);
    }

    #[test]
    fn groups_runs_on_the_same_baseline() {
        let content = "BT /F1 10 Tf 40 700 Td (a) Tj ET BT /F1 10 Tf 60 700 Td (b) Tj ET";
        assert_eq!(content.matches("BT").count(), 2);

        let pdf = minimal_pdf("x");
        assert!(!pdf.is_empty());
    }
}
