use gix::ObjectId;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

/// `.git/gmap/cache`: a string table, then one record per commit, little-endian.
/// A file that fails to parse is ignored and rebuilt, so bump this to change the format.
const MAGIC: &[u8; 8] = b"gmap\x00\x00\x00\x02";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Add,
    Delete,
    Modify,
    Rename,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawChange {
    pub kind: Kind,
    pub path: String,
    /// The path before a rename.
    pub old: Option<String>,
    pub added: u32,
    pub deleted: u32,
    pub binary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCommit {
    pub time: i64,
    /// The author's UTC offset in seconds.
    pub tz: i32,
    pub name: String,
    pub email: String,
    pub summary: String,
    /// The parent could not be read (a shallow clone), so `changes` is empty.
    pub partial: bool,
    pub changes: Vec<RawChange>,
}

pub type Cache = HashMap<ObjectId, RawCommit>;

pub fn load(path: &Path) -> Cache {
    std::fs::read(path).ok().and_then(|b| decode(&b)).unwrap_or_default()
}

pub fn save(path: &Path, cache: &Cache) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    f.write_all(&encode(cache))?;
    f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(&tmp, path)
}

fn encode(cache: &Cache) -> Vec<u8> {
    let mut strings: HashMap<&str, u32> = HashMap::new();
    let mut table: Vec<&str> = Vec::new();
    for c in cache.values() {
        for s in [c.name.as_str(), c.email.as_str()]
            .into_iter()
            .chain(c.changes.iter().flat_map(|ch| std::iter::once(ch.path.as_str()).chain(ch.old.as_deref())))
        {
            if !strings.contains_key(s) {
                strings.insert(s, table.len() as u32);
                table.push(s);
            }
        }
    }

    let mut out = Vec::with_capacity(64 + cache.len() * 96);
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, table.len() as u32);
    for s in &table {
        put_str(&mut out, s);
    }
    put_u32(&mut out, cache.len() as u32);
    for (id, c) in cache {
        let bytes = id.as_bytes();
        out.push(bytes.len() as u8);
        out.extend_from_slice(bytes);
        out.extend_from_slice(&c.time.to_le_bytes());
        out.extend_from_slice(&c.tz.to_le_bytes());
        put_u32(&mut out, strings[c.name.as_str()]);
        put_u32(&mut out, strings[c.email.as_str()]);
        put_str(&mut out, &c.summary);
        out.push(c.partial as u8);
        put_u32(&mut out, c.changes.len() as u32);
        for ch in &c.changes {
            let kind = match ch.kind {
                Kind::Add => 0u8,
                Kind::Delete => 1,
                Kind::Modify => 2,
                Kind::Rename => 3,
            };
            out.push(kind | (ch.binary as u8) << 7);
            put_u32(&mut out, strings[ch.path.as_str()]);
            put_u32(&mut out, ch.old.as_deref().map_or(u32::MAX, |o| strings[o]));
            put_u32(&mut out, ch.added);
            put_u32(&mut out, ch.deleted);
        }
    }
    out
}

fn decode(b: &[u8]) -> Option<Cache> {
    let mut r = Reader { b, at: 0 };
    if r.take(8)? != MAGIC {
        return None;
    }
    let n = r.u32()? as usize;
    let mut table = Vec::with_capacity(n.min(1 << 20));
    for _ in 0..n {
        table.push(r.str()?);
    }
    let get = |i: u32| table.get(i as usize).cloned();
    let n = r.u32()? as usize;
    let mut cache = Cache::with_capacity(n.min(1 << 22));
    for _ in 0..n {
        let len = r.take(1)?[0] as usize;
        let id = ObjectId::try_from(r.take(len)?).ok()?;
        let time = i64::from_le_bytes(r.take(8)?.try_into().ok()?);
        let tz = i32::from_le_bytes(r.take(4)?.try_into().ok()?);
        let name = get(r.u32()?)?;
        let email = get(r.u32()?)?;
        let summary = r.str()?;
        let partial = r.take(1)?[0] != 0;
        let m = r.u32()? as usize;
        let mut changes = Vec::with_capacity(m.min(1 << 16));
        for _ in 0..m {
            let k = r.take(1)?[0];
            let kind = match k & 0x7f {
                0 => Kind::Add,
                1 => Kind::Delete,
                2 => Kind::Modify,
                3 => Kind::Rename,
                _ => return None,
            };
            let path = get(r.u32()?)?;
            let old = match r.u32()? {
                u32::MAX => None,
                i => Some(get(i)?),
            };
            let added = r.u32()?;
            let deleted = r.u32()?;
            changes.push(RawChange { kind, path, old, added, deleted, binary: k & 0x80 != 0 });
        }
        cache.insert(id, RawCommit { time, tz, name, email, summary, partial, changes });
    }
    (r.at == b.len()).then_some(cache)
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Cache {
        let mut c = Cache::new();
        let id = ObjectId::from_hex(b"1234567890abcdef1234567890abcdef12345678").unwrap();
        c.insert(
            id,
            RawCommit {
                time: 1_700_000_000,
                tz: -7 * 3600,
                name: "Ada".into(),
                email: "ada@example.com".into(),
                summary: "Fix the thing ✓".into(),
                partial: false,
                changes: vec![
                    RawChange {
                        kind: Kind::Modify,
                        path: "src/a.rs".into(),
                        old: None,
                        added: 3,
                        deleted: 1,
                        binary: false,
                    },
                    RawChange {
                        kind: Kind::Rename,
                        path: "src/b.rs".into(),
                        old: Some("src/a.rs".into()),
                        added: 0,
                        deleted: 0,
                        binary: false,
                    },
                    RawChange {
                        kind: Kind::Add,
                        path: "logo.png".into(),
                        old: None,
                        added: 0,
                        deleted: 0,
                        binary: true,
                    },
                ],
            },
        );
        c
    }

    #[test]
    fn round_trip() {
        let c = sample();
        assert_eq!(decode(&encode(&c)).unwrap(), c);
    }

    #[test]
    fn rejects_damage() {
        let b = encode(&sample());
        for cut in [0, 7, 8, 20, b.len() - 1] {
            assert!(decode(&b[..cut]).is_none(), "cut at {cut}");
        }
        let mut bad = b.clone();
        bad[3] ^= 1;
        assert!(decode(&bad).is_none());
    }
}
