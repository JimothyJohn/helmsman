//! A small non-validating XML reader, enough for `PLCopen` TC6 files:
//! elements, attributes, text, CDATA, comments, processing instructions and
//! the predefined and numeric character references. A DOCTYPE is refused,
//! which also rules out entity expansion. Namespaces are not resolved:
//! names keep their local part (`xhtml:p` is `p`) and `xmlns` attributes
//! are dropped.

use crate::lexer::Pos;

/// Deepest element nesting accepted.
pub const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Elem(Element),
    /// Character data and its position (of the first character).
    Text(String, Pos),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    pub pos: Pos,
    pub msg: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Child elements in document order.
    pub fn elements(&self) -> impl Iterator<Item = &Self> {
        self.children.iter().filter_map(|n| match n {
            Node::Elem(e) => Some(e),
            Node::Text(..) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&Self> {
        self.elements().find(|e| e.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Self> {
        self.elements().filter(move |e| e.name == name)
    }

    /// All character data below this element, concatenated, with the
    /// position of the first non-blank piece.
    pub fn text(&self) -> (String, Option<Pos>) {
        let mut s = String::new();
        let mut first = None;
        let mut stack = vec![self.children.iter()];
        while let Some(it) = stack.last_mut() {
            match it.next() {
                None => {
                    stack.pop();
                }
                Some(Node::Elem(e)) => stack.push(e.children.iter()),
                Some(Node::Text(t, p)) => {
                    if first.is_none() && !t.trim().is_empty() {
                        first = Some(*p);
                    }
                    s.push_str(t);
                }
            }
        }
        (s, first)
    }
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

const fn name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':' || !c.is_ascii()
}

const fn name_char(c: char) -> bool {
    name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

struct Reader<'a> {
    src: &'a str,
    i: usize,
    line: u32,
    col: u32,
}

type R<T> = Result<T, XmlError>;

impl Reader<'_> {
    const fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(XmlError {
            pos: self.pos(),
            msg: msg.into(),
        })
    }

    fn rest(&self) -> &str {
        &self.src[self.i..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.rest().starts_with(s) {
            for _ in s.chars() {
                self.bump();
            }
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    /// Consumes through `end`; the text before it is returned.
    fn until(&mut self, end: &str, what: &str) -> R<String> {
        let start = self.pos();
        let Some(n) = self.rest().find(end) else {
            return Err(XmlError {
                pos: start,
                msg: format!("unterminated {what}"),
            });
        };
        let s = self.rest()[..n].to_string();
        for _ in s.chars().chain(end.chars()) {
            self.bump();
        }
        Ok(s)
    }

    fn name(&mut self) -> R<String> {
        match self.peek() {
            Some(c) if name_start(c) => {}
            Some(c) => return self.err(format!("expected a name, found {c:?}")),
            None => return self.err("expected a name, found the end of the file"),
        }
        let mut s = String::new();
        while let Some(c) = self.peek().filter(|&c| name_char(c)) {
            s.push(c);
            self.bump();
        }
        Ok(s)
    }

    /// Text up to `stop` (exclusive), with references decoded.
    fn chars(&mut self, stop: &[char], attr: bool) -> R<String> {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if stop.contains(&c) {
                break;
            }
            if c == '&' {
                s.push(self.reference()?);
                continue;
            }
            if attr && c == '<' {
                return self.err("'<' in an attribute value");
            }
            self.bump();
            // Attribute-value normalisation (XML 1.0 3.3.3).
            s.push(if attr && matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            });
        }
        Ok(s)
    }

    fn reference(&mut self) -> R<char> {
        let start = self.pos();
        self.bump();
        let body = self.until(";", "character reference")?;
        let bad = |msg: String| XmlError { pos: start, msg };
        let code = match body.as_str() {
            "lt" => return Ok('<'),
            "gt" => return Ok('>'),
            "amp" => return Ok('&'),
            "quot" => return Ok('"'),
            "apos" => return Ok('\''),
            b => {
                let n = if let Some(h) = b.strip_prefix("#x") {
                    u32::from_str_radix(h, 16).ok()
                } else if let Some(d) = b.strip_prefix('#') {
                    d.parse().ok()
                } else {
                    return Err(bad(format!("unknown entity &{b}; (no DTD is read)")));
                };
                n.filter(|_| b.len() <= 10)
                    .ok_or_else(|| bad(format!("bad character reference &{b};")))?
            }
        };
        char::from_u32(code)
            .filter(|&c| c != '\0')
            .ok_or_else(|| bad(format!("bad character reference &{body};")))
    }

    /// Comments, processing instructions and whitespace between elements.
    /// A DOCTYPE is refused.
    fn misc(&mut self) -> R<()> {
        loop {
            self.skip_ws();
            if self.eat("<!--") {
                self.until("-->", "comment")?;
            } else if self.rest().starts_with("<!") {
                return self.err("DOCTYPE and other declarations are not accepted");
            } else if self.eat("<?") {
                self.until("?>", "processing instruction")?;
            } else {
                return Ok(());
            }
        }
    }

    /// After `<`: the element, its name as written, and whether it is
    /// self-closing.
    fn open_tag(&mut self, pos: Pos) -> R<(Element, String, bool)> {
        let name = self.name()?;
        let mut attrs: Vec<(String, String)> = Vec::new();
        loop {
            let had_ws = self.peek().is_some_and(char::is_whitespace);
            self.skip_ws();
            if self.eat("/>") {
                return Ok((elem(&name, attrs, pos), name, true));
            }
            if self.eat(">") {
                return Ok((elem(&name, attrs, pos), name, false));
            }
            if !had_ws {
                return self.err(format!("malformed tag <{name}>"));
            }
            let an = self.name()?;
            self.skip_ws();
            if !self.eat("=") {
                return self.err(format!("attribute {an} has no value"));
            }
            self.skip_ws();
            let Some(q @ ('"' | '\'')) = self.bump() else {
                return self.err(format!("attribute {an}: value must be quoted"));
            };
            let v = self.chars(&[q], true)?;
            if self.bump().is_none() {
                return self.err("unterminated attribute value");
            }
            if an == "xmlns" || an.starts_with("xmlns:") {
                continue;
            }
            let an = local(&an).to_string();
            if attrs.iter().any(|(n, _)| *n == an) {
                return self.err(format!("attribute {an} given twice"));
            }
            attrs.push((an, v));
        }
    }
}

fn elem(name: &str, attrs: Vec<(String, String)>, pos: Pos) -> Element {
    Element {
        name: local(name).to_string(),
        attrs,
        children: Vec::new(),
        pos,
    }
}

/// Attaches a finished element to its parent; the root ends the document.
fn finish(r: &mut Reader, open: &mut [(Element, String)], e: Element) -> R<Option<Element>> {
    if let Some((parent, _)) = open.last_mut() {
        parent.children.push(Node::Elem(e));
        return Ok(None);
    }
    r.misc()?;
    if r.i < r.src.len() {
        return r.err("content after the root element");
    }
    Ok(Some(e))
}

/// Parses a document into its root element.
///
/// # Errors
/// The first well-formedness problem found.
pub fn parse(src: &str) -> Result<Element, XmlError> {
    let mut r = Reader {
        src: src.strip_prefix('\u{feff}').unwrap_or(src),
        i: 0,
        line: 1,
        col: 1,
    };
    r.misc()?;
    if !r.rest().starts_with('<') {
        return r.err("expected the root element");
    }
    // Open elements, innermost last; the iteration keeps hostile nesting
    // off the call stack.
    let mut open: Vec<(Element, String)> = Vec::new();
    loop {
        let pos = r.pos();
        if r.eat("</") {
            let name = r.name()?;
            r.skip_ws();
            if !r.eat(">") {
                return r.err(format!("malformed end tag </{name}>"));
            }
            let Some((done, raw)) = open.pop() else {
                return Err(XmlError {
                    pos,
                    msg: format!("end tag </{name}> without a start tag"),
                });
            };
            if raw != name {
                return Err(XmlError {
                    pos,
                    msg: format!("</{name}> closes <{raw}> (line {})", done.pos.line),
                });
            }
            if let Some(root) = finish(&mut r, &mut open, done)? {
                return Ok(root);
            }
        } else if r.eat("<!--") {
            r.until("-->", "comment")?;
        } else if r.eat("<![CDATA[") {
            let at = r.pos();
            let t = r.until("]]>", "CDATA section")?;
            let Some((parent, _)) = open.last_mut() else {
                return r.err("CDATA outside the root element");
            };
            parent.children.push(Node::Text(t, at));
        } else if r.eat("<?") {
            r.until("?>", "processing instruction")?;
        } else if r.rest().starts_with("<!") {
            return r.err("declarations are not accepted inside elements");
        } else if r.eat("<") {
            if open.len() >= MAX_DEPTH {
                return r.err(format!("elements nested deeper than {MAX_DEPTH}"));
            }
            let (e, raw, closed) = r.open_tag(pos)?;
            if closed {
                if let Some(root) = finish(&mut r, &mut open, e)? {
                    return Ok(root);
                }
            } else {
                open.push((e, raw));
            }
        } else if r.peek().is_none() {
            let name = open.last().map_or("", |(_, n)| n.as_str());
            return r.err(format!("<{name}> is not closed"));
        } else {
            let t = r.chars(&['<'], false)?;
            if let Some((parent, _)) = open.last_mut() {
                parent.children.push(Node::Text(t, pos));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(src: &str) -> String {
        parse(src).unwrap_err().msg
    }

    #[test]
    fn elements_attributes_text() {
        let e = parse(
            "<?xml version='1.0' encoding='utf-8'?>\n<!-- c -->\n\
             <a:project xmlns:a='x' xmlns=\"y\" n='1 &lt; 2'>\
             <b x=\"&#65;&#x42;\"/> t&amp;u <![CDATA[<ST>&amp;]]><c><d>e</d></c></a:project>",
        )
        .unwrap();
        assert_eq!(e.name, "project");
        assert_eq!(e.attrs, vec![("n".to_string(), "1 < 2".to_string())]);
        assert_eq!(e.child("b").unwrap().attr("x"), Some("AB"));
        assert_eq!(e.text().0, " t&u <ST>&amp;e");
        assert_eq!(e.child("c").unwrap().pos, Pos { line: 3, col: 97 });
    }

    #[test]
    fn text_position_is_the_first_non_blank_piece() {
        let e = parse("<a>\n  <b><![CDATA[x\ny]]></b></a>").unwrap();
        assert_eq!(e.text(), ("\n  x\ny".to_string(), Some(Pos { line: 2, col: 15 })));
    }

    #[test]
    fn malformed_documents_are_refused() {
        assert!(err("").contains("root element"));
        assert!(err("<a>").contains("<a> is not closed"));
        assert!(err("<a></b>").contains("closes <a>"));
        assert!(err("<a/><b/>").contains("after the root"));
        assert!(err("<a x=1/>").contains("quoted"));
        assert!(err("<a x='1' x='2'/>").contains("twice"));
        assert!(err("<a x='<'/>").contains("'<'"));
        assert!(err("<a>&nbsp;</a>").contains("unknown entity"));
        assert!(err("<a>&#0;</a>").contains("bad character"));
        assert!(err("<a>&#xD800;</a>").contains("bad character"));
        assert!(err("<a>&#99999999999;</a>").contains("bad character"));
        assert!(err("<a><!-- </a>").contains("unterminated comment"));
        assert!(err("<a x='1'y='2'/>").contains("malformed"));
    }

    #[test]
    fn doctype_and_entities_are_refused() {
        let laughs = "<!DOCTYPE l [<!ENTITY a \"aaaa\"><!ENTITY b \"&a;&a;\">]><l>&b;</l>";
        assert!(err(laughs).contains("DOCTYPE"));
        assert!(err("<a><!ENTITY x 'y'></a>").contains("declarations"));
    }

    #[test]
    fn nesting_is_bounded() {
        let ok = "<a>".repeat(MAX_DEPTH) + &"</a>".repeat(MAX_DEPTH);
        parse(&ok).unwrap();
        let deep = "<a>".repeat(100_000);
        assert!(err(&deep).contains("nested deeper"));
    }
}
