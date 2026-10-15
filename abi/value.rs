//! ABI 1.1 value tree wire format. No Rust layouts cross the boundary.
//! One-byte tags: 0 NULL, 1 u32 LE, 2 UTF-8, 3 bytes, 4 array, 5 string map.
//! Spans and element counts have a u32 LE prefix. Maps alternate a UTF-8
//! length-prefixed key (without a tag) and a tagged value. No trailing bytes.
//! Limits: 64 MiB total bytes, 64 nesting levels, 1 million nodes.
#[derive(Clone, Debug, PartialEq)]
pub enum Tree {
    Null,
    Number(u32),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<Tree>),
    Map(Vec<(String, Tree)>),
}
pub const LIMIT: usize = 64 * 1024 * 1024;
impl Tree {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        fn span(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), String> {
            let len = u32::try_from(bytes.len()).map_err(|_| "value span too large")?;
            out.extend(len.to_le_bytes());
            out.extend(bytes);
            Ok(())
        }
        fn put(v: &Tree, out: &mut Vec<u8>, depth: usize, nodes: &mut usize) -> Result<(), String> {
            *nodes += 1;
            if depth > 64 || *nodes > 1_000_000 {
                return Err("native value tree exceeds limits".into());
            }
            match v {
                Tree::Null => out.push(0),
                Tree::Number(n) => {
                    out.push(1);
                    out.extend(n.to_le_bytes());
                }
                Tree::Text(s) => {
                    out.push(2);
                    span(out, s.as_bytes())?;
                }
                Tree::Bytes(b) => {
                    out.push(3);
                    span(out, b)?;
                }
                Tree::Array(a) => {
                    out.push(4);
                    out.extend((a.len() as u32).to_le_bytes());
                    for v in a {
                        put(v, out, depth + 1, nodes)?;
                    }
                }
                Tree::Map(m) => {
                    out.push(5);
                    out.extend((m.len() as u32).to_le_bytes());
                    for (k, v) in m {
                        span(out, k.as_bytes())?;
                        put(v, out, depth + 1, nodes)?;
                    }
                }
            }
            if out.len() > LIMIT {
                return Err("native value tree exceeds 64 MiB".into());
            }
            Ok(())
        }
        let mut out = Vec::new();
        put(self, &mut out, 0, &mut 0)?;
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
            if n > b.len() {
                return Err("truncated native value tree".into());
            }
            let (a, rest) = b.split_at(n);
            *b = rest;
            Ok(a)
        }
        fn count(b: &mut &[u8]) -> Result<usize, String> {
            Ok(u32::from_le_bytes(take(b, 4)?.try_into().unwrap()) as usize)
        }
        fn text(b: &mut &[u8]) -> Result<String, String> {
            let n = count(b)?;
            String::from_utf8(take(b, n)?.to_vec()).map_err(|e| e.to_string())
        }
        fn get(b: &mut &[u8], depth: usize, nodes: &mut usize) -> Result<Tree, String> {
            *nodes += 1;
            if depth > 64 || *nodes > 1_000_000 {
                return Err("native value tree exceeds limits".into());
            }
            Ok(match take(b, 1)?[0] {
                0 => Tree::Null,
                1 => Tree::Number(count(b)? as u32),
                2 => Tree::Text(text(b)?),
                3 => {
                    let n = count(b)?;
                    Tree::Bytes(take(b, n)?.to_vec())
                }
                tag @ (4 | 5) => {
                    let n = count(b)?;
                    if n > 1_000_000 || n > b.len() {
                        return Err("invalid native value count".into());
                    }
                    if tag == 4 {
                        Tree::Array(
                            (0..n)
                                .map(|_| get(b, depth + 1, nodes))
                                .collect::<Result<_, _>>()?,
                        )
                    } else {
                        let mut seen = std::collections::BTreeSet::new();
                        let mut values = Vec::new();
                        for _ in 0..n {
                            let k = text(b)?;
                            if !seen.insert(k.clone()) {
                                return Err("duplicate native map key".into());
                            }
                            values.push((k, get(b, depth + 1, nodes)?));
                        }
                        Tree::Map(values)
                    }
                }
                _ => return Err("invalid native value tag".into()),
            })
        }
        if bytes.len() > LIMIT {
            return Err("native value tree exceeds 64 MiB".into());
        }
        let mut remaining = bytes;
        let value = get(&mut remaining, 0, &mut 0)?;
        if !remaining.is_empty() {
            return Err("trailing native value bytes".into());
        }
        Ok(value)
    }
}
