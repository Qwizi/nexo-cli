//! WooCommerce HTML (WYSIWYG descriptions, spec tables) to the CMS rich text
//! (ProseMirror JSON): paragraphs, H2–H4, lists, quotes, tables, line
//! breaks, bold/italic/underline/strike and links. Everything else keeps
//! its text and loses its markup; images and scripts are dropped. Links to
//! the shop's own hosts become site paths (`/regulamin/`).

use scraper::{ElementRef, Html, Node};
use serde_json::{Value, json};

/// A rich text document; `None` when the HTML holds no text. `hosts` are
/// the shop's own hosts (lowercase).
pub fn to_doc(html: &str, hosts: &[String]) -> Option<Value> {
    let fragment = Html::parse_fragment(html);
    let mut builder = Builder::new(hosts);
    builder.children(fragment.root_element(), &[]);
    let blocks = builder.finish();
    (!blocks.is_empty()).then(|| json!({ "type": "doc", "content": blocks }))
}

/// Plain text of an HTML snippet (entities decoded, spaces collapsed).
pub fn text(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    collapse(&fragment.root_element().text().collect::<String>())
        .trim()
        .to_owned()
}

fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

fn link(href: &str, hosts: &[String]) -> Option<String> {
    let href = href.trim();
    if href.starts_with('/')
        || href.starts_with('#')
        || href.starts_with("mailto:")
        || href.starts_with("tel:")
    {
        return Some(href.to_owned());
    }
    let rest = href
        .strip_prefix("https://")
        .or_else(|| href.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if hosts.iter().any(|own| own.eq_ignore_ascii_case(host)) {
        return Some(format!("/{path}"));
    }
    href.starts_with("https://").then(|| href.to_owned())
}

struct Builder<'a> {
    hosts: &'a [String],
    blocks: Vec<Value>,
    inline: Vec<Value>,
}

impl<'a> Builder<'a> {
    fn new(hosts: &'a [String]) -> Self {
        Self {
            hosts,
            blocks: Vec::new(),
            inline: Vec::new(),
        }
    }

    fn finish(mut self) -> Vec<Value> {
        self.flush();
        self.blocks
    }

    /// Closes the open paragraph (trimmed; empty ones vanish).
    fn flush(&mut self) {
        let inline = trim(std::mem::take(&mut self.inline));
        if !inline.is_empty() {
            self.blocks
                .push(json!({ "type": "paragraph", "content": inline }));
        }
    }

    fn text(&mut self, text: &str, marks: &[Value]) {
        let text = collapse(text);
        if text.is_empty() {
            return;
        }
        // Merge with the previous run of the same formatting.
        if let Some(last) = self.inline.last_mut()
            && last["type"] == "text"
            && last.get("marks").map_or(marks.is_empty(), |m| {
                m.as_array().is_some_and(|m| m == marks)
            })
            && let Some(previous) = last["text"].as_str()
        {
            let merged = format!("{previous}{text}").replace("  ", " ");
            last["text"] = json!(merged);
            return;
        }
        let mut node = json!({ "type": "text", "text": text });
        if !marks.is_empty() {
            node["marks"] = json!(marks);
        }
        self.inline.push(node);
    }

    fn children(&mut self, element: ElementRef<'_>, marks: &[Value]) {
        for child in element.children() {
            match child.value() {
                Node::Text(text) => self.text(text, marks),
                Node::Element(_) => {
                    if let Some(el) = ElementRef::wrap(child) {
                        self.element(el, marks);
                    }
                }
                _ => {}
            }
        }
    }

    /// Blocks made of an element's children (list items, cells, quotes).
    fn nested(&self, element: ElementRef<'_>) -> Vec<Value> {
        let mut inner = Builder::new(self.hosts);
        inner.children(element, &[]);
        inner.finish()
    }

    fn element(&mut self, element: ElementRef<'_>, marks: &[Value]) {
        let tag = element.value().name();
        let with = |mark: Value| {
            let mut marks = marks.to_vec();
            if !marks.contains(&mark) {
                marks.push(mark);
            }
            marks
        };
        match tag {
            "script" | "style" | "img" | "svg" | "iframe" | "noscript" => {}
            "br" => self.inline.push(json!({ "type": "hardBreak" })),
            "strong" | "b" => self.children(element, &with(json!({ "type": "bold" }))),
            "em" | "i" => self.children(element, &with(json!({ "type": "italic" }))),
            "u" => self.children(element, &with(json!({ "type": "underline" }))),
            "s" | "strike" | "del" => self.children(element, &with(json!({ "type": "strike" }))),
            "a" => match element
                .value()
                .attr("href")
                .and_then(|h| link(h, self.hosts))
            {
                Some(href) => self.children(
                    element,
                    &with(json!({ "type": "link", "attrs": { "href": href } })),
                ),
                None => self.children(element, marks),
            },
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let level = match tag {
                    "h1" | "h2" => 2,
                    "h3" => 3,
                    _ => 4,
                };
                let mut inner = Builder::new(self.hosts);
                inner.children(element, &[]);
                let content = trim(inner.inline);
                if !content.is_empty() {
                    self.blocks.push(
                        json!({ "type": "heading", "attrs": { "level": level }, "content": content }),
                    );
                }
            }
            "ul" | "ol" => {
                self.flush();
                let items: Vec<Value> = element
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter_map(|item| {
                        let blocks = self.nested(item);
                        (!blocks.is_empty())
                            .then(|| json!({ "type": "listItem", "content": blocks }))
                    })
                    .collect();
                if !items.is_empty() {
                    let kind = if tag == "ol" {
                        "orderedList"
                    } else {
                        "bulletList"
                    };
                    self.blocks.push(json!({ "type": kind, "content": items }));
                }
            }
            "blockquote" => {
                self.flush();
                let blocks = self.nested(element);
                if !blocks.is_empty() {
                    self.blocks
                        .push(json!({ "type": "blockquote", "content": blocks }));
                }
            }
            "hr" => {
                self.flush();
                self.blocks.push(json!({ "type": "horizontalRule" }));
            }
            "table" => {
                self.flush();
                let rows: Vec<Value> = element
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .filter(|e| e.value().name() == "tr")
                    .map(|row| {
                        let cells: Vec<Value> = row
                            .children()
                            .filter_map(ElementRef::wrap)
                            .filter(|c| matches!(c.value().name(), "td" | "th"))
                            .map(|cell| {
                                let kind = if cell.value().name() == "th" {
                                    "tableHeader"
                                } else {
                                    "tableCell"
                                };
                                let mut blocks = self.nested(cell);
                                if blocks.is_empty() {
                                    blocks.push(json!({ "type": "paragraph" }));
                                }
                                json!({ "type": kind, "content": blocks })
                            })
                            .collect();
                        json!({ "type": "tableRow", "content": cells })
                    })
                    .filter(|row| row["content"].as_array().is_some_and(|c| !c.is_empty()))
                    .collect();
                if !rows.is_empty() {
                    self.blocks
                        .push(json!({ "type": "table", "content": rows }));
                }
            }
            "p" | "div" | "section" | "article" | "header" | "footer" | "li" | "dd" | "dt"
            | "figure" | "figcaption" | "address" => {
                self.flush();
                self.children(element, marks);
                self.flush();
            }
            _ => self.children(element, marks),
        }
    }
}

/// Drops spaces at the ends of a paragraph and breaks at its edges.
fn trim(mut inline: Vec<Value>) -> Vec<Value> {
    while inline.first().is_some_and(|n| n["type"] == "hardBreak") {
        inline.remove(0);
    }
    while inline.last().is_some_and(|n| n["type"] == "hardBreak") {
        inline.pop();
    }
    if let Some(first) = inline.first_mut()
        && let Some(text) = first["text"].as_str()
    {
        first["text"] = json!(text.trim_start());
    }
    if let Some(last) = inline.last_mut()
        && let Some(text) = last["text"].as_str()
    {
        last["text"] = json!(text.trim_end());
    }
    inline.retain(|n| n["type"] != "text" || n["text"].as_str().is_some_and(|t| !t.is_empty()));
    inline
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn spec_sheets_and_lists() {
        let doc = to_doc(
            "<h2>Sofa Cavelle</h2><h4>Wymiary:</h4><p>Szerokość: <strong>164 cm</strong><br>Wysokość: 70 cm</p>\
             <div><ul><li><span>Stelaż: drewno</span></li><li> <em>Tkanina</em>  boucle </li></ul></div>",
            &[],
        )
        .unwrap();
        let blocks = doc["content"].as_array().unwrap();
        assert_eq!(
            blocks[0],
            json!({ "type": "heading", "attrs": { "level": 2 }, "content": [{ "type": "text", "text": "Sofa Cavelle" }] })
        );
        assert_eq!(blocks[1]["attrs"]["level"], 4);
        assert_eq!(
            blocks[2]["content"],
            json!([
                { "type": "text", "text": "Szerokość: " },
                { "type": "text", "text": "164 cm", "marks": [{ "type": "bold" }] },
                { "type": "hardBreak" },
                { "type": "text", "text": "Wysokość: 70 cm" }
            ])
        );
        let list = &blocks[3];
        assert_eq!(list["type"], "bulletList");
        assert_eq!(
            list["content"][0]["content"][0]["content"][0]["text"],
            "Stelaż: drewno"
        );
        assert_eq!(
            list["content"][1]["content"][0]["content"],
            json!([
                { "type": "text", "text": "Tkanina", "marks": [{ "type": "italic" }] },
                { "type": "text", "text": " boucle" }
            ])
        );
    }

    #[test]
    fn tables_headings_and_links() {
        let doc = to_doc(
            "<h1>Informacje</h1><table><tbody><tr><th>Waga</th><td>28 kg</td></tr><tr><th>Kolor</th><td></td></tr></tbody></table>\
             <p><a href=\"http://sklep.example.test/regulamin/\">Regulamin</a> i <a href=\"javascript:x\">zły link</a></p>",
            &["sklep.example.test".to_owned()],
        )
        .unwrap();
        let blocks = doc["content"].as_array().unwrap();
        assert_eq!(blocks[0]["attrs"]["level"], 2, "H1 becomes H2");
        let table = &blocks[1];
        assert_eq!(table["content"][0]["content"][0]["type"], "tableHeader");
        assert_eq!(
            table["content"][0]["content"][1]["content"][0]["content"][0]["text"],
            "28 kg"
        );
        assert_eq!(
            table["content"][1]["content"][1]["content"],
            json!([{ "type": "paragraph" }])
        );
        let paragraph = &blocks[2]["content"];
        assert_eq!(paragraph[0]["marks"][0]["attrs"]["href"], "/regulamin/");
        assert!(
            paragraph[1].get("marks").is_none(),
            "unsafe links lose the link"
        );
    }

    #[test]
    fn empty_and_plain_text() {
        assert_eq!(to_doc("<p>  </p><div><br></div>", &[]), None);
        assert_eq!(
            text("Sofa &#8211; nowoczesna &amp; wygodna"),
            "Sofa – nowoczesna & wygodna"
        );
        let doc = to_doc("Zwykły tekst", &[]).unwrap();
        assert_eq!(doc["content"][0]["content"][0]["text"], "Zwykły tekst");
    }
}
