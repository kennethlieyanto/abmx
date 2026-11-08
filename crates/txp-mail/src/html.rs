//! HTML to ordered visible blocks.
//!
//! The goal is not readable text, it is *structure*: one block per label and one
//! per value, in document order.  Banks render label/value tables, so splitting
//! on block-level elements makes "value comes after its label" reliable even
//! when the layout changes.

use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// Elements that start a new visible block.
///
/// `span` is included on purpose: bank templates use inline spans to lay out
/// "Rp" / "100.000" / ",00" as separate nodes, and keeping them separate makes
/// the amount extractor's job trivial.
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "tr",
    "td",
    "th",
    "span",
    "li",
    "ul",
    "ol",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "br",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "section",
    "article",
    "header",
    "footer",
    "blockquote",
    "pre",
    "hr",
    "dt",
    "dd",
    "form",
    "nav",
    "figure",
    "figcaption",
    "main",
    "aside",
    "option",
];

/// Elements whose contents are not visible text.
const SKIP_TAGS: &[&str] = &["script", "style", "head", "title", "noscript", "template"];

pub fn blocks_from_html(html: &str) -> Vec<String> {
    let dom = match parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
    {
        Ok(dom) => dom,
        Err(_) => return Vec::new(),
    };

    let mut blocks = Vec::new();
    let mut buffer = String::new();
    walk(&dom.document, &mut blocks, &mut buffer);
    flush(&mut buffer, &mut blocks);
    blocks
}

fn walk(node: &Handle, blocks: &mut Vec<String>, buffer: &mut String) {
    let tag = match &node.data {
        NodeData::Element { name, .. } => Some(name.local.to_string()),
        _ => None,
    };

    if let Some(tag) = tag.as_deref() {
        if SKIP_TAGS.contains(&tag) {
            return;
        }
    }

    let is_block = tag
        .as_deref()
        .map(|tag| BLOCK_TAGS.contains(&tag))
        .unwrap_or(false);

    if is_block {
        flush(buffer, blocks);
    }

    match &node.data {
        NodeData::Text { contents } => {
            let text = contents.borrow();
            buffer.push_str(&text);
        }
        _ => {
            for child in node.children.borrow().iter() {
                walk(child, blocks, buffer);
            }
        }
    }

    if is_block {
        flush(buffer, blocks);
    }
}

fn flush(buffer: &mut String, blocks: &mut Vec<String>) {
    if buffer.is_empty() {
        return;
    }
    let normalized = crate::normalize_block(buffer);
    if !normalized.is_empty() {
        blocks.push(normalized);
    }
    buffer.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_cells_become_separate_blocks() {
        let html = r#"<table><tr><td>Merchant</td><td>:</td><td>ExamplePay</td></tr></table>"#;
        assert_eq!(blocks_from_html(html), vec!["Merchant", "ExamplePay"]);
    }

    #[test]
    fn inline_spans_keep_amount_parts_apart() {
        let html = r#"<div><span>Rp</span><span>100.000</span><span>,00</span></div>"#;
        assert_eq!(blocks_from_html(html), vec!["Rp", "100.000", ",00"]);
    }

    #[test]
    fn skips_style_and_script() {
        let html = r#"<style>.x{color:red}</style><script>var a=1</script><div>Total</div>"#;
        assert_eq!(blocks_from_html(html), vec!["Total"]);
    }

    #[test]
    fn breaks_on_br() {
        let html = "<div>Hai <strong>Andi</strong>,<br>Ada transaksi</div>";
        assert_eq!(blocks_from_html(html), vec!["Hai Andi,", "Ada transaksi"]);
    }
}
