//! A small JSON value and its writer (no dependencies): `reda --json`'s output.
//! Objects keep their keys in insertion order, so the same report prints the same bytes.

#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn obj() -> J { J::Obj(vec![]) }

    /// add `key: value` to an object (builder style)
    pub fn with(mut self, key: &str, v: impl Into<J>) -> J {
        self.set(key, v);
        self
    }

    /// set `key` in an object: replaces it if present, else appends it
    pub fn set(&mut self, key: &str, v: impl Into<J>) {
        if let J::Obj(kv) = self {
            let v = v.into();
            match kv.iter_mut().find(|(k, _)| k == key) {
                Some(e) => e.1 = v,
                None => kv.push((key.to_string(), v)),
            }
        }
    }

    /// `key: value` only when `v` is `Some`
    pub fn with_opt<T: Into<J>>(self, key: &str, v: Option<T>) -> J {
        match v { Some(v) => self.with(key, v), None => self }
    }

    /// two-space indented text, a newline at the end
    pub fn pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, 0);
        s.push('\n');
        s
    }

    fn write(&self, s: &mut String, ind: usize) {
        match self {
            J::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
            J::Int(n) => s.push_str(&n.to_string()),
            J::Num(x) if x.is_finite() => s.push_str(&x.to_string()),
            J::Num(_) => s.push_str("null"),
            J::Str(t) => quote(s, t),
            J::Arr(xs) if xs.is_empty() => s.push_str("[]"),
            J::Obj(kv) if kv.is_empty() => s.push_str("{}"),
            J::Arr(xs) => {
                s.push('[');
                for (i, x) in xs.iter().enumerate() {
                    s.push_str(if i == 0 { "\n" } else { ",\n" });
                    pad(s, ind + 1);
                    x.write(s, ind + 1);
                }
                s.push('\n');
                pad(s, ind);
                s.push(']');
            }
            J::Obj(kv) => {
                s.push('{');
                for (i, (k, v)) in kv.iter().enumerate() {
                    s.push_str(if i == 0 { "\n" } else { ",\n" });
                    pad(s, ind + 1);
                    quote(s, k);
                    s.push_str(": ");
                    v.write(s, ind + 1);
                }
                s.push('\n');
                pad(s, ind);
                s.push('}');
            }
        }
    }
}

fn pad(s: &mut String, ind: usize) {
    for _ in 0..ind { s.push_str("  "); }
}

fn quote(s: &mut String, t: &str) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

impl From<bool> for J { fn from(b: bool) -> J { J::Bool(b) } }
impl From<i64> for J { fn from(n: i64) -> J { J::Int(n) } }
impl From<i32> for J { fn from(n: i32) -> J { J::Int(n as i64) } }
impl From<u32> for J { fn from(n: u32) -> J { J::Int(n as i64) } }
impl From<usize> for J { fn from(n: usize) -> J { J::Int(n as i64) } }
impl From<f64> for J { fn from(x: f64) -> J { J::Num(x) } }
impl From<&str> for J { fn from(t: &str) -> J { J::Str(t.to_string()) } }
impl From<String> for J { fn from(t: String) -> J { J::Str(t) } }
impl From<&String> for J { fn from(t: &String) -> J { J::Str(t.clone()) } }
impl<T: Into<J>> From<Vec<T>> for J { fn from(xs: Vec<T>) -> J { J::Arr(xs.into_iter().map(Into::into).collect()) } }

#[cfg(test)]
mod tests {
    use super::J;

    #[test]
    fn writes() {
        let j = J::obj().with("a", 1i64).with("s", "x\"\n\u{1}").with("e", J::Arr(vec![])).with("l", vec![true, false]);
        assert_eq!(j.pretty(), "{\n  \"a\": 1,\n  \"s\": \"x\\\"\\n\\u0001\",\n  \"e\": [],\n  \"l\": [\n    true,\n    false\n  ]\n}\n");
        let mut k = J::obj().with("a", 1i64);
        k.set("a", 2i64);
        assert_eq!(k, J::obj().with("a", 2i64));
    }
}
