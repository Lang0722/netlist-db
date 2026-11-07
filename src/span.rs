use core::{fmt, num::NonZeroU32, ops::Index};
use std::{
    collections::{HashMap, hash_map::Entry},
    path::{Path, PathBuf},
};

pub fn span(file_ctx: &str, line_offset: u32) -> LocatedSpan<'_> {
    unsafe { LocatedSpan::new_from_raw_offset(0, 1 + line_offset, file_ctx, ()) }
}
pub type LocatedSpan<'a> = nom_locate::LocatedSpan<&'a str>;

/// A text in a file. Make one with [`Span::new`]: the end is `start + len`, so
/// a change of `start` alone moves the end too.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub start: usize,
    pub len: u32,
    /// The line number of the fragment relatively to the input of the parser. It starts at line 1.
    pub line_num: NonZeroU32,
}

// A big deck is mostly spans. As `&str` has this size, a built list takes the
// memory of its builder list.
const _: () = assert!(size_of::<Span>() == size_of::<&str>());

impl Default for Span {
    fn default() -> Self {
        Self::new(0, 0, 1)
    }
}

impl Span {
    /// The text from byte `start` to byte `end`, on the line `line_num` (the
    /// first line is 1). A span holds at most 4 GiB. Only two texts can be
    /// longer, and only in a file over 4 GiB: the text of a `.subckt` and the
    /// rest of the file after `.alter`. The parser cuts them with
    /// [`Span::capped_len`].
    #[inline]
    pub fn new(start: usize, end: usize, line_num: u32) -> Self {
        Self {
            start,
            len: u32::try_from(end - start).unwrap_or(u32::MAX),
            line_num: NonZeroU32::new(line_num).unwrap_or(NonZeroU32::MIN),
        }
    }

    /// Where the text ends
    #[inline]
    pub fn end(&self) -> usize {
        self.start + self.len as usize
    }

    /// The length of `text`, cut at 4 GiB (the most that a span holds) and
    /// moved back to a char boundary, so that the text can be sliced there.
    pub fn capped_len(text: &str) -> usize {
        let mut len = text.len().min(u32::MAX as usize);
        while !text.is_char_boundary(len) {
            len -= 1;
        }
        len
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Pos {
    pub start: usize,
    pub line_num: u32,
}

impl Pos {
    pub fn new(i: LocatedSpan) -> Option<Self> {
        if let Ok((_, pos)) = nom_locate::position::<LocatedSpan, ()>(i) {
            Some(pos.into())
        } else {
            None
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum FileId {
    Include { path: PathBuf },
    Section { path: PathBuf, section: String },
}

impl From<PathBuf> for FileId {
    fn from(path: PathBuf) -> Self {
        Self::Include { path }
    }
}

impl From<(PathBuf, String)> for FileId {
    fn from(value: (PathBuf, String)) -> Self {
        Self::Section {
            path: value.0,
            section: value.1,
        }
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileId::Include { path } => write!(f, "File \"{}\"", path.display()),
            FileId::Section { path, section } => {
                write!(f, "File \"{}\", section {section}", path.display())
            }
        }
    }
}

impl FileId {
    pub fn path(&self) -> &Path {
        match self {
            FileId::Include { path } | FileId::Section { path, section: _ } => path,
        }
    }

    /// This file (or section) by the canonical form of its path (`..`
    /// and links resolved), when that form is another path. `None` also
    /// when the file cannot be found.
    pub fn canonical(&self) -> Option<FileId> {
        let canon = std::fs::canonicalize(self.path()).ok()?;
        (canon != self.path()).then(|| match self {
            FileId::Include { .. } => FileId::Include { path: canon },
            FileId::Section { section, .. } => FileId::Section {
                path: canon,
                section: section.clone(),
            },
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParsedId(pub usize);

#[derive(Debug)]
pub struct FileStorage<Parsed> {
    pub file: Vec<String>,
    pub parsed: Vec<(FileId, Parsed)>,
    pub id2idx: HashMap<FileId, ParsedId>,
}
impl<Parsed> Default for FileStorage<Parsed> {
    #[inline]
    fn default() -> Self {
        const CAP: usize = 4;
        Self {
            id2idx: HashMap::with_capacity(CAP),
            file: Vec::with_capacity(CAP),
            parsed: Vec::with_capacity(CAP),
        }
    }
}
impl<Parsed: Default> FileStorage<Parsed> {
    pub fn existed(&self, file_id: &FileId) -> Option<ParsedId> {
        crate::info!("load {file_id}");
        self.id2idx.get(file_id).copied()
    }
    pub fn new_file(&mut self, file_id: FileId) -> ParsedId {
        match self.id2idx.entry(file_id) {
            Entry::Occupied(occupied) => {
                crate::warn!("already loaded {:?}", occupied.key());
                *occupied.get()
            }
            Entry::Vacant(vacant_entry) => {
                let files_num = self.file.len();
                self.file.push(String::new());
                self.parsed
                    .push((vacant_entry.key().clone(), Parsed::default()));
                *vacant_entry.insert(ParsedId(files_num))
            }
        }
    }
    /// Adds `alias` as another key of the file `id`: the same file reached
    /// by a path spelled another way. An existing key is kept.
    pub fn alias(&mut self, alias: FileId, id: ParsedId) {
        self.id2idx.entry(alias).or_insert(id);
    }
    pub fn update_ctx(&mut self, parsed_id: &ParsedId, file_ctx: String, parsed: Parsed) {
        self.file[parsed_id.0] = file_ctx;
        self.parsed[parsed_id.0].1 = parsed;
    }
}

impl Index<&Span> for str {
    type Output = str;
    #[inline]
    fn index(&self, index: &Span) -> &Self::Output {
        &self[index.start..index.end()]
    }
}

impl Index<&Span> for String {
    type Output = str;
    #[inline]
    fn index(&self, index: &Span) -> &Self::Output {
        &self[index.start..index.end()]
    }
}

impl From<LocatedSpan<'_>> for Span {
    #[inline]
    fn from(s: LocatedSpan<'_>) -> Self {
        let start = s.location_offset();
        Self::new(start, start + s.fragment().len(), s.location_line())
    }
}

impl From<LocatedSpan<'_>> for Pos {
    #[inline]
    fn from(s: LocatedSpan<'_>) -> Self {
        let start = s.location_offset();
        Self {
            start,
            line_num: s.location_line(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use nom::{
        IResult,
        bytes::complete::{tag, take_until},
    };

    /// A text longer than 4 GiB is cut at 4 GiB, and does not wrap round.
    #[test]
    fn long_span_is_cut() {
        let span = Span::new(10, 10 + (5 << 30), 1);
        assert_eq!(span.end(), 10 + u32::MAX as usize);
        assert_eq!(Span::new(3, 7, 2).end(), 7);
    }

    #[derive(Debug, Default)]
    struct Token {
        // pub pos: Pos,
        pub _foo: Span,
        pub _bar: Span,
    }

    fn parse_foobar(s: LocatedSpan) -> IResult<LocatedSpan, Token> {
        let (s, _) = take_until("foo")(s)?;
        let (s, foo) = tag("foo")(s)?;
        let (s, bar) = tag("bar")(s)?;
        Ok((
            s,
            Token {
                _foo: foo.into(),
                _bar: bar.into(),
            },
        ))
    }
    #[test]
    fn main() {
        let mut file_storage = FileStorage::default();
        let file_ctx = String::from("Lorem ipsum \n foobar");
        let parsed_id = file_storage.new_file(FileId::Include {
            path: "dummy.sp".into(),
        });
        let input = span(&file_ctx, 0);
        let output = parse_foobar(input).unwrap().1;
        file_storage.update_ctx(&parsed_id, file_ctx, output);
        let file = &file_storage.file[parsed_id.0];
        println!("{}", &file[&file_storage.parsed[parsed_id.0].1._foo]);
    }
}
