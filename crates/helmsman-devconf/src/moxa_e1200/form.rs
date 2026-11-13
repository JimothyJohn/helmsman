//! A small, bounded reader for the console's HTML forms, and the rules a
//! browser applies when it submits one.
//!
//! It understands `form`, `input`, `select` and `option` (tag and
//! attribute names in any case), skips comments and `script`/`style`/
//! `textarea`/`title` bodies (a script that writes `<input ...>` is not a
//! field), and refuses a page that goes past its limits instead of
//! truncating a value it would later write back.
//!
//! The submission rules are the Python client's
//! (`scripts/moxa-e1200-console.py` in helmsman-devices):
//! unticked checkboxes and unselected radios are left out; `IdleTime` is
//! left out unless `EnableIDLE` is ticked and `WDTime` unless `EnableWD`
//! is (the page's script disables them); buttons other than the one named
//! `Submit` are left out, because they act on their own; a static
//! `disabled` attribute is ignored (a page's script is not run, so it is
//! not known which fields it would disable).

use super::Error;

/// Forms read from one page.
pub const MAX_FORMS: usize = 16;
/// Fields (inputs and selects) in one form.
pub const MAX_FIELDS: usize = 512;
/// Options in one select.
pub const MAX_OPTIONS: usize = 256;
/// Longest field name.
pub const MAX_NAME: usize = 128;
/// Longest field value or option, in characters.
pub const MAX_VALUE: usize = 4096;
/// Attributes read from one tag.
const MAX_ATTRS: usize = 32;

/// Fields a page's script disables while the checkbox beside them is
/// unticked (`04_1.htm`'s `checkWDValue()`).
const DISABLED_UNLESS: [(&str, &str); 2] = [("IdleTime", "EnableIDLE"), ("WDTime", "EnableWD")];

/// What kind of control a field is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Hidden,
    Password,
    Checkbox,
    Radio,
    /// The one submit button named `Submit`.
    Submit,
    Select,
}

/// One named control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    /// What a browser would send now; `None` when it sends nothing (an
    /// unticked checkbox or radio).
    pub value: Option<String>,
    /// A checkbox's or radio's value when ticked (`on` when the page gives
    /// none, as a browser does).
    pub on_value: String,
    /// A select's option values, in order.
    pub options: Vec<String>,
}

/// One `<form>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// The `action` attribute as written.
    pub action: String,
    /// The `method` attribute, lowercase (`get` when absent).
    pub method: String,
    pub fields: Vec<Field>,
}

fn too_much(what: &'static str) -> Error {
    Error::Page(what)
}

/// Every form on `html`, in order.
///
/// # Errors
/// [`Error::Page`] when the page goes past a limit above.
pub fn parse(html: &str) -> Result<Vec<Form>, Error> {
    let mut b = Builder::default();
    let s = html.as_bytes();
    let mut i = 0;
    while i < s.len() {
        let Some(rel) = s[i..].iter().position(|&c| c == b'<') else {
            b.text(&html[i..]);
            break;
        };
        let lt = i + rel;
        b.text(&html[i..lt]);
        let rest = &s[lt..];
        if rest.starts_with(b"<!--") {
            i = find(s, b"-->", lt + 4).map_or(s.len(), |k| k + 3);
        } else if rest.get(1).is_some_and(|&c| c == b'/' || c.is_ascii_alphabetic()) {
            let closing = rest[1] == b'/';
            let name_at = lt + 1 + usize::from(closing);
            let name_end = name_at
                + s[name_at..]
                    .iter()
                    .position(|&c| !(c.is_ascii_alphanumeric() || c == b'-'))
                    .unwrap_or(s.len() - name_at);
            let name = html[name_at..name_end].to_ascii_lowercase();
            let (attrs, end) = read_attrs(html, name_end);
            i = end;
            if closing {
                b.end_tag(&name)?;
            } else {
                b.start_tag(&name, &attrs)?;
                if matches!(name.as_str(), "script" | "style" | "textarea" | "title") {
                    let close = format!("</{name}");
                    i = find(s, close.as_bytes(), i).unwrap_or(s.len());
                }
            }
        } else if rest.get(1).is_some_and(|&c| c == b'!' || c == b'?') {
            i = s[lt..]
                .iter()
                .position(|&c| c == b'>')
                .map_or(s.len(), |k| lt + k + 1);
        } else {
            b.text("<");
            i = lt + 1;
        }
    }
    b.finish()
}

/// First index of `needle` in `hay` at or after `from`, ignoring ASCII case.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&k| hay[k..k + needle.len()].eq_ignore_ascii_case(needle))
}

/// Attributes from `at` to the closing `>` (quotes may hide a `>`); the
/// index after it. Names lowercase, values with entities decoded.
fn read_attrs(html: &str, at: usize) -> (Vec<(String, String)>, usize) {
    let s = html.as_bytes();
    let mut i = at;
    let mut out = Vec::new();
    while i < s.len() {
        match s[i] {
            b'>' => return (out, i + 1),
            c if c.is_ascii_whitespace() || c == b'/' => i += 1,
            _ => {
                let start = i;
                while i < s.len() && !(s[i].is_ascii_whitespace() || matches!(s[i], b'=' | b'>' | b'/')) {
                    i += 1;
                }
                // A stray `=` at the start would make no progress.
                if i == start {
                    i += 1;
                    continue;
                }
                let name = html[start..i].to_ascii_lowercase();
                while i < s.len() && s[i].is_ascii_whitespace() {
                    i += 1;
                }
                let mut value = String::new();
                if i < s.len() && s[i] == b'=' {
                    i += 1;
                    while i < s.len() && s[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if i < s.len() && (s[i] == b'"' || s[i] == b'\'') {
                        let q = s[i];
                        let vs = i + 1;
                        let ve = s[vs..].iter().position(|&c| c == q).map_or(s.len(), |k| vs + k);
                        value = decode(&html[vs..ve]);
                        i = (ve + 1).min(s.len());
                    } else {
                        let vs = i;
                        while i < s.len() && !(s[i].is_ascii_whitespace() || s[i] == b'>') {
                            i += 1;
                        }
                        value = decode(&html[vs..i]);
                    }
                }
                if out.len() < MAX_ATTRS {
                    out.push((name, value));
                }
            }
        }
    }
    (out, s.len())
}

/// The entities a settings page can plausibly carry.
fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(k) = rest.find('&') {
        out.push_str(&rest[..k]);
        rest = &rest[k..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let ent = &rest[1..e];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => ent.strip_prefix('#').and_then(|n| {
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(code)
                }),
            }?;
            Some((ch, e + 1))
        });
        if let Some((ch, used)) = decoded {
            out.push(ch);
            rest = &rest[used..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Builder {
    done: Vec<Form>,
    cur: Option<Form>,
    /// Index in `cur.fields` of the open select.
    sel: Option<usize>,
    /// An open `<option>` that has no `value` attribute: its text is the value.
    opt: Option<OptionText>,
}

struct OptionText {
    selected: bool,
    text: String,
}

impl Builder {
    fn text(&mut self, t: &str) {
        if let Some(o) = self.opt.as_mut().filter(|o| o.text.len() <= MAX_VALUE) {
            o.text.push_str(&decode(t));
        }
    }

    fn start_tag(&mut self, name: &str, attrs: &[(String, String)]) -> Result<(), Error> {
        let attr = |n: &str| attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        let has = |n: &str| attrs.iter().any(|(k, _)| k == n);
        match name {
            "form" => {
                self.close_form()?;
                if self.done.len() < MAX_FORMS {
                    self.cur = Some(Form {
                        action: attr("action").unwrap_or("").to_string(),
                        method: attr("method").unwrap_or("get").to_ascii_lowercase(),
                        fields: Vec::new(),
                    });
                } else {
                    return Err(too_much("more forms on the page than are read"));
                }
            }
            "input" => {
                self.close_option()?;
                let ty = attr("type").unwrap_or("text").to_ascii_lowercase();
                let Some(fname) = attr("name").filter(|n| !n.is_empty()) else {
                    return Ok(());
                };
                let value = attr("value").unwrap_or("");
                let (kind, val) = match ty.as_str() {
                    "button" | "file" | "image" | "reset" => return Ok(()),
                    "submit" => {
                        if fname != "Submit" {
                            return Ok(()); // other submit buttons act on their own
                        }
                        (Kind::Submit, Some(value.to_string()))
                    }
                    "hidden" => (Kind::Hidden, Some(value.to_string())),
                    "password" => (Kind::Password, Some(value.to_string())),
                    "checkbox" | "radio" => {
                        let on = if value.is_empty() && attr("value").is_none() {
                            "on"
                        } else {
                            value
                        };
                        let kind = if ty == "radio" {
                            Kind::Radio
                        } else {
                            Kind::Checkbox
                        };
                        let f = Field {
                            name: fname.to_string(),
                            kind,
                            value: has("checked").then(|| on.to_string()),
                            on_value: on.to_string(),
                            options: Vec::new(),
                        };
                        return self.push(f);
                    }
                    _ => (Kind::Text, Some(value.to_string())),
                };
                check_len(fname, MAX_NAME)?;
                check_len(val.as_deref().unwrap_or(""), MAX_VALUE)?;
                self.push(Field {
                    name: fname.to_string(),
                    kind,
                    value: val,
                    on_value: String::new(),
                    options: Vec::new(),
                })?;
            }
            "select" => {
                self.close_select()?;
                let Some(fname) = attr("name").filter(|n| !n.is_empty()) else {
                    return Ok(());
                };
                self.push(Field {
                    name: fname.to_string(),
                    kind: Kind::Select,
                    value: None,
                    on_value: String::new(),
                    options: Vec::new(),
                })?;
                self.sel = self.cur.as_ref().map(|f| f.fields.len() - 1);
            }
            "option" => {
                self.close_option()?;
                if self.sel.is_none() {
                    return Ok(());
                }
                match attr("value") {
                    Some(v) => self.add_option(v.to_string(), has("selected"))?,
                    None => {
                        self.opt = Some(OptionText {
                            selected: has("selected"),
                            text: String::new(),
                        });
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn end_tag(&mut self, name: &str) -> Result<(), Error> {
        match name {
            "option" => self.close_option(),
            "select" => self.close_select(),
            "form" => self.close_form(),
            _ => Ok(()),
        }
    }

    fn push(&mut self, f: Field) -> Result<(), Error> {
        check_len(&f.name, MAX_NAME)?;
        check_len(&f.on_value, MAX_VALUE)?;
        if let Some(form) = &mut self.cur {
            if form.fields.len() >= MAX_FIELDS {
                return Err(too_much("more fields in a form than are read"));
            }
            form.fields.push(f);
        }
        Ok(())
    }

    fn add_option(&mut self, value: String, selected: bool) -> Result<(), Error> {
        check_len(&value, MAX_VALUE)?;
        if let (Some(form), Some(i)) = (&mut self.cur, self.sel) {
            let f = &mut form.fields[i];
            if f.options.len() >= MAX_OPTIONS {
                return Err(too_much("more options in a select than are read"));
            }
            if selected {
                f.value = Some(value.clone());
            }
            f.options.push(value);
        }
        Ok(())
    }

    fn close_option(&mut self) -> Result<(), Error> {
        if let Some(o) = self.opt.take() {
            self.add_option(o.text.trim().to_string(), o.selected)?;
        }
        Ok(())
    }

    fn close_select(&mut self) -> Result<(), Error> {
        self.close_option()?;
        if let (Some(form), Some(i)) = (&mut self.cur, self.sel.take()) {
            let f = &mut form.fields[i];
            if f.value.is_none() {
                f.value = f.options.first().cloned(); // a browser shows the first
            }
        }
        Ok(())
    }

    fn close_form(&mut self) -> Result<(), Error> {
        self.close_select()?;
        if let Some(f) = self.cur.take() {
            self.done.push(f);
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<Form>, Error> {
        self.close_form()?;
        Ok(self.done)
    }
}

fn check_len(s: &str, max: usize) -> Result<(), Error> {
    if s.chars().count() > max {
        Err(too_much(
            "a field name or value on the page is longer than is read",
        ))
    } else {
        Ok(())
    }
}

impl Form {
    /// The first field called `name`.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// What a browser would send for `name` now.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.name == name && f.value.is_some())
            .and_then(|f| f.value.as_deref())
    }

    /// The `(name, value)` pairs a browser sends, in document order.
    #[must_use]
    pub fn submitted(&self) -> Vec<(String, String)> {
        let ticked = |n: &str| {
            self.fields
                .iter()
                .any(|f| f.name == n && matches!(f.kind, Kind::Checkbox | Kind::Radio) && f.value.is_some())
        };
        self.fields
            .iter()
            .filter_map(|f| {
                let v = f.value.as_ref()?;
                if DISABLED_UNLESS.iter().any(|&(n, by)| n == f.name && !ticked(by)) {
                    return None;
                }
                Some((f.name.clone(), v.clone()))
            })
            .collect()
    }

    /// The query string of [`Form::submitted`].
    #[must_use]
    pub fn query(&self) -> String {
        query(&self.submitted())
    }

    /// Changes fields as a person would on the page. A checkbox takes `1`
    /// (ticked), an empty value or `0` (unticked); a select takes one of its options.
    /// `token` and `code` belong to the page and cannot be set. Nothing is
    /// changed unless every change is accepted.
    ///
    /// # Errors
    /// [`Error::Refused`] naming the field (never a value).
    pub fn apply(&mut self, changes: &[(&str, &str)]) -> Result<(), Error> {
        let mut next = self.clone();
        for &(name, value) in changes {
            next.apply_one(name, value)?;
        }
        *self = next;
        Ok(())
    }

    fn apply_one(&mut self, name: &str, value: &str) -> Result<(), Error> {
        if name == "token" || name == "code" {
            return Err(Error::Refused(format!("{name}: set by the page, not by hand")));
        }
        if !self.fields.iter().any(|f| f.name == name) {
            let known: Vec<&str> = self.fields.iter().map(|f| f.name.as_str()).collect();
            return Err(Error::Refused(format!(
                "{name}: not a field of this form ({})",
                known.join(", ")
            )));
        }
        if value.chars().count() > MAX_VALUE || value.chars().any(|c| c.is_control() || u32::from(c) > 0xFF) {
            return Err(Error::Refused(format!(
                "{name}: a value is Latin-1 text of at most {MAX_VALUE} characters, no control characters"
            )));
        }
        let kind = self.fields.iter().find(|f| f.name == name).map(|f| f.kind);
        match kind {
            Some(Kind::Submit) => return Err(Error::Refused(format!("{name}: a button, not a setting"))),
            Some(Kind::Checkbox) => {
                let on = !matches!(value, "" | "0");
                for f in self.fields.iter_mut().filter(|f| f.name == name) {
                    f.value = on.then(|| f.on_value.clone());
                }
            }
            Some(Kind::Radio) => {
                if !self.fields.iter().any(|f| f.name == name && f.on_value == value) {
                    return Err(Error::Refused(format!(
                        "{name}: not one of this radio group's values"
                    )));
                }
                for f in self.fields.iter_mut().filter(|f| f.name == name) {
                    f.value = (f.on_value == value).then(|| f.on_value.clone());
                }
            }
            Some(Kind::Select) => {
                let f = self
                    .fields
                    .iter_mut()
                    .find(|f| f.name == name)
                    .ok_or_else(|| Error::Refused(format!("{name}: not a field")))?;
                if !f.options.iter().any(|o| o == value) {
                    return Err(Error::Refused(format!(
                        "{name}: not one of its options ({})",
                        f.options.join(", ")
                    )));
                }
                f.value = Some(value.to_string());
            }
            _ => {
                if let Some(f) = self.fields.iter_mut().find(|f| f.name == name) {
                    f.value = Some(value.to_string());
                }
            }
        }
        Ok(())
    }
}

/// `application/x-www-form-urlencoded`, the way a browser writes it: space
/// is `+`, letters, digits and `_.-~` stay, everything else is `%XX` of the
/// Latin-1 byte (a character past U+00FF, which a page cannot hold, as its
/// UTF-8 bytes).
#[must_use]
pub fn encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push('+'),
            c if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '~') => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                match u8::try_from(u32::from(c)) {
                    Ok(b) => {
                        let _ = write!(out, "%{b:02X}");
                    }
                    Err(_) => {
                        for b in c.encode_utf8(&mut buf).bytes() {
                            let _ = write!(out, "%{b:02X}");
                        }
                    }
                }
            }
        }
    }
    out
}

/// `name=value` pairs joined with `&`, each [`encode`]d.
#[must_use]
pub fn query(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const PAGE: &str = r#"<HTML><BODY>
<!-- <form action="evil.htm"><input name=ghost value=1> -->
<FORM method="GET" action="set.htm">
<INPUT type=hidden name=token value="T0K">
<input type="text" NAME="MOD_DESC" value="a &amp; b">
<input type="text" name="MOD_LOC" value=''>
<input type="checkbox" name="EnableIDLE" value="1" checked>
<input type="text" name="IdleTime" value="60">
<input type="checkbox" name="EnableWD" value="1">
<input type="text" name="WDTime" value="5">
<input type="checkbox" name="EnableSafeModeAutoClear" value="1">
<select name="SetSM00"><option value="0">Off<option value="1" selected>On</select>
<select name="SetSM01"><option>Low<option>High</select>
<input type="submit" name="Clear_Safe_Flag1" value="Clear">
<input type="button" name="b" value="x">
<input type="submit" name="Submit" value="Submit">
<input type="hidden" name="code" value="41">
</FORM>
<script>document.write('<input name="fromscript" value="1">');</script>
</BODY></HTML>"#;

    fn page_form() -> Form {
        let forms = parse(PAGE).unwrap();
        assert_eq!(
            forms.len(),
            1,
            "the comment's form and the script's input are not fields"
        );
        forms.into_iter().next().unwrap()
    }

    #[test]
    fn reads_the_form_as_a_browser_submits_it() {
        let f = page_form();
        assert_eq!(f.action, "set.htm");
        assert_eq!(f.method, "get");
        assert_eq!(
            f.submitted(),
            [
                ("token", "T0K"),
                ("MOD_DESC", "a & b"),
                ("MOD_LOC", ""),
                ("EnableIDLE", "1"),
                ("IdleTime", "60"),
                // EnableWD unticked: no EnableWD, no WDTime. Unticked box: none.
                ("SetSM00", "1"),
                ("SetSM01", "Low"),
                ("Submit", "Submit"),
                ("code", "41"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
    }

    #[test]
    fn the_documented_write_is_what_the_form_builds() {
        let mut f = page_form();
        f.apply(&[("MOD_LOC", "helmsman bench"), ("SetSM00", "0")])
            .unwrap();
        f.apply(&[("SetSM00", "1")]).unwrap();
        assert_eq!(
            f.query(),
            "token=T0K&MOD_DESC=a+%26+b&MOD_LOC=helmsman+bench&EnableIDLE=1&IdleTime=60&SetSM00=1&SetSM01=Low&Submit=Submit&code=41"
        );
    }

    #[test]
    fn unticking_a_checkbox_drops_its_dependent_field() {
        let mut f = page_form();
        f.apply(&[("EnableIDLE", "0")]).unwrap();
        assert!(!f.query().contains("IdleTime"));
        assert!(!f.query().contains("EnableIDLE"));
        f.apply(&[("EnableWD", "1")]).unwrap();
        assert!(f.query().contains("EnableWD=1&WDTime=5"));
    }

    #[test]
    fn refuses_what_the_page_does_not_have() {
        let mut f = page_form();
        let before = f.clone();
        for bad in [
            ("token", "x"),
            ("code", "42"),
            ("nope", "1"),
            ("Submit", "x"),
            ("SetSM00", "2"),
            ("MOD_LOC", "line\nbreak"),
            ("MOD_LOC", "\u{4e2d}"),
        ] {
            assert!(matches!(f.apply(&[bad]), Err(Error::Refused(_))), "{bad:?}");
        }
        // One bad change in a batch applies none of them.
        assert!(f.apply(&[("MOD_LOC", "ok"), ("nope", "1")]).is_err());
        assert_eq!(f, before);
    }

    #[test]
    fn radio_groups_tick_one() {
        let forms = parse(
            "<form><input type=radio name=m value=a checked><input type=radio name=m value=b><input name=code value=1></form>",
        )
        .unwrap();
        let mut f = forms.into_iter().next().unwrap();
        f.apply(&[("m", "b")]).unwrap();
        assert_eq!(f.submitted()[0], ("m".into(), "b".into()));
        assert!(f.apply(&[("m", "c")]).is_err());
    }

    #[test]
    fn checkbox_without_a_value_sends_on() {
        let f = parse("<form><input type=checkbox name=x checked></form>")
            .unwrap()
            .remove(0);
        assert_eq!(f.submitted(), [("x".to_string(), "on".to_string())]);
    }

    #[test]
    fn encoding_matches_a_browser() {
        assert_eq!(encode("a b&c=d/é~_.-"), "a+b%26c%3Dd%2F%E9~_.-");
        assert_eq!(encode("\u{4e2d}"), "%E4%B8%AD");
    }

    #[test]
    fn limits_are_errors_not_truncation() {
        let big = format!("<form>{}</form>", "<input name=a value=b>".repeat(MAX_FIELDS + 1));
        assert!(parse(&big).is_err());
        let long = format!(
            "<form><input name=a value=\"{}\"></form>",
            "x".repeat(MAX_VALUE + 1)
        );
        assert!(parse(&long).is_err());
        let forms = "<form></form>".repeat(MAX_FORMS + 1);
        assert!(parse(&forms).is_err());
        let opts = format!(
            "<form><select name=s>{}</select></form>",
            "<option>x".repeat(MAX_OPTIONS + 1)
        );
        assert!(parse(&opts).is_err());
    }

    #[test]
    fn broken_markup_is_survivable() {
        for s in [
            "<",
            "<form",
            "<form action=\"x",
            "<form><input name=\"a",
            "<input =>",
            "<script>",
            "<form><select name=s><option",
            "&#xFFFFFFFFFF;&#;&;&amp",
            "<!--",
            "<!doctype",
            "</",
        ] {
            let _ = parse(s);
        }
    }

    proptest! {
        #[test]
        fn arbitrary_text_never_panics(s in "\\PC{0,3000}") {
            if let Ok(forms) = parse(&s) { check_bounds(&forms); }
        }

        #[test]
        fn tag_soup_never_panics_and_stays_bounded(
            s in "(<[a-zA-Z/!?]{0,8}( [a-zA-Z]{1,8}(=(\"[^\"]{0,12}\"|'[^']{0,12}'|[a-z0-9]{0,6}))?){0,5}/?>|[a-z &#;0-9]{0,12}){0,120}"
        ) {
            if let Ok(mut forms) = parse(&s) {
                check_bounds(&forms);
                for f in &mut forms {
                    let _ = f.query();
                    let _ = f.apply(&[("a", "b"), ("token", "t")]);
                }
            }
        }

        #[test]
        fn encoded_values_are_url_safe(s in "\\PC{0,200}") {
            let e = encode(&s);
            prop_assert!(e.bytes().all(|b| b.is_ascii_alphanumeric() || b"%+_.-~".contains(&b)));
        }
    }

    fn check_bounds(forms: &[Form]) {
        assert!(forms.len() <= MAX_FORMS);
        for f in forms {
            assert!(f.fields.len() <= MAX_FIELDS);
            for x in &f.fields {
                assert!(x.name.chars().count() <= MAX_NAME);
                assert!(x.options.len() <= MAX_OPTIONS);
                assert!(x.value.as_deref().unwrap_or("").chars().count() <= MAX_VALUE);
            }
        }
    }
}
