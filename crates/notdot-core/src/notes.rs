//! Private, user-authored voice notes and lists.
//!
//! Notes are separate from learned memories, conversation history and
//! reminders. Requests are recognised by a deliberately narrow, deterministic
//! grammar on this computer; nothing here asks a model to choose a target.
//! Wording is kept as spoken. A follow-up such as "add oat milk to that list"
//! only uses an exact, recently focused note; otherwise notdot asks.
use serde::{Deserialize, Serialize};

pub const MAX_NOTES: usize = 200;
pub const MAX_ITEMS: usize = 100;
pub const MAX_TITLE_CHARS: usize = 80;
pub const MAX_ITEM_BYTES: usize = 300;
pub const MAX_BODY_BYTES: usize = 4000;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Upper bound for a stored revision, leaving room for one more change.
pub const MAX_REVISION: u64 = u64::MAX - 1;
/// How long "that list" keeps referring to the last note you used.
pub const FOCUS_SECONDS: u64 = 120;
/// How long a spoken delete or rewrite confirmation stays valid.
pub const CONFIRM_SECONDS: u64 = 120;
/// How long a creation question ("What should I call it?") waits for an answer.
pub const DRAFT_SECONDS: u64 = 60;
const SPOKEN_LIMIT: usize = 1500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Note,
    List,
}
impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Note => "note",
            Kind::List => "list",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub text: String,
    pub done: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub items: Vec<Item>,
    pub created: u64,
    pub updated: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notebook {
    pub version: u32,
    pub revision: u64,
    pub notes: Vec<Note>,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}

/// A title is spoken text shown in Settings and export, never a path.
pub fn clean_title(title: &str) -> Result<String, String> {
    let title = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’' | ',' | '.'))
        .trim()
        .to_owned();
    if title.is_empty() {
        return Err("Say a name for the note or list.".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err("That name is too long. Use at most 80 characters.".into());
    }
    if title
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    {
        return Err("That name contains characters I can't save.".into());
    }
    Ok(title)
}
fn clean_text(text: &str, limit: usize) -> Result<String, String> {
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', ','])
        .trim()
        .to_owned();
    if text.is_empty() {
        return Err("I didn't hear what to save.".into());
    }
    if text.len() > limit {
        return Err(format!(
            "That is too long to save. Keep it under {limit} characters."
        ));
    }
    if text.chars().any(|c| c.is_control()) {
        return Err("That contains characters I can't save.".into());
    }
    Ok(text)
}

/// Lowercase words without punctuation, leading articles or a trailing kind.
/// Matching is deterministic and independent of capitalisation.
pub fn normalise(text: &str) -> String {
    let lower = text.to_lowercase().replace('’', "'");
    let mut words: Vec<&str> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty())
        .collect();
    while words
        .first()
        .is_some_and(|w| matches!(*w, "the" | "my" | "a" | "an" | "our"))
        && words.len() > 1
    {
        words.remove(0);
    }
    while words
        .last()
        .is_some_and(|w| matches!(*w, "list" | "note" | "notes" | "lists"))
        && words.len() > 1
    {
        words.pop();
    }
    words.join(" ")
}

impl Notebook {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 && !(self.version == 0 && self.notes.is_empty()) {
            return Err("This notes file uses an unsupported format".into());
        }
        if self.notes.len() > MAX_NOTES {
            return Err("Too many notes".into());
        }
        // Every change advances the revision; an exhausted counter could
        // otherwise wrap and revive a stale confirmation.
        if self.revision >= MAX_REVISION {
            return Err("This notes file has an invalid revision".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for note in &self.notes {
            if !valid_id(&note.id) || !ids.insert(note.id.as_str()) {
                return Err("A note has an invalid or repeated ID".into());
            }
            if clean_title(&note.title).as_deref() != Ok(note.title.as_str()) {
                return Err("A note has an invalid title".into());
            }
            if note.body.len() > MAX_BODY_BYTES
                || note.items.len() > MAX_ITEMS
                || (note.kind == Kind::List && !note.body.is_empty())
                || (note.kind == Kind::Note && !note.items.is_empty())
            {
                return Err("A note is larger than notdot allows".into());
            }
            for item in &note.items {
                if !valid_id(&item.id)
                    || !ids.insert(item.id.as_str())
                    || item.text.is_empty()
                    || item.text.len() > MAX_ITEM_BYTES
                    || item.text.chars().any(char::is_control)
                {
                    return Err("A list item is invalid".into());
                }
            }
            if note.body.chars().any(|c| c.is_control() && c != '\n') {
                return Err("A note contains invalid characters".into());
            }
        }
        Ok(())
    }
    fn get(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }
    fn get_mut(&mut self, id: &str) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }
    fn titles(&self) -> String {
        spoken_join(self.notes.iter().map(|n| n.title.as_str()).collect())
    }
    /// Exact normalised title, then a unique note containing every spoken word.
    fn find(&self, title: &str, kind: Option<Kind>) -> Lookup {
        let wanted = normalise(title);
        if wanted.is_empty() {
            return Lookup::Missing;
        }
        let exact: Vec<_> = self
            .notes
            .iter()
            .filter(|n| normalise(&n.title) == wanted)
            .collect();
        let pick = |found: Vec<&Note>| -> Option<Lookup> {
            match found.as_slice() {
                [] => None,
                [one] => Some(Lookup::Found(one.id.clone())),
                many => {
                    let typed: Vec<_> = many.iter().filter(|n| Some(n.kind) == kind).collect();
                    if let [one] = typed.as_slice() {
                        Some(Lookup::Found(one.id.clone()))
                    } else {
                        Some(Lookup::Ambiguous(
                            many.iter().map(|n| describe(n)).collect(),
                        ))
                    }
                }
            }
        };
        if let Some(found) = pick(exact) {
            return found;
        }
        let words: Vec<_> = wanted.split(' ').collect();
        let partial: Vec<_> = self
            .notes
            .iter()
            .filter(|n| {
                let title = normalise(&n.title);
                let have: Vec<_> = title.split(' ').collect();
                words.iter().all(|w| have.contains(w))
            })
            .collect();
        pick(partial).unwrap_or(Lookup::Missing)
    }
}
enum Lookup {
    Found(String),
    Ambiguous(Vec<String>),
    Missing,
}
fn describe(note: &Note) -> String {
    format!("{} ({})", note.title, note.kind.word())
}
fn spoken_join(parts: Vec<&str>) -> String {
    match parts.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// "that list", "it": only the exact focused note, never a guess.
    That(Option<Kind>),
    Named {
        title: String,
        kind: Option<Kind>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `title: None` starts a short dialogue asking for the name.
    Create {
        kind: Kind,
        title: Option<String>,
        content: Option<String>,
    },
    Add {
        target: Target,
        text: String,
    },
    Read {
        target: Target,
    },
    ListAll,
    Find {
        query: String,
    },
    Complete {
        target: Target,
        item: String,
        done: bool,
    },
    RemoveItem {
        target: Target,
        item: String,
    },
    Rename {
        target: Target,
        title: String,
    },
    Delete {
        target: Target,
    },
    Replace {
        target: Target,
        text: String,
    },
}

/// Prepare speech for matching while keeping the original wording available.
/// Keywords are ASCII, so an ASCII-lowercase copy keeps identical byte offsets.
fn prepare(text: &str) -> String {
    let mut text = text
        .replace(['’', '‘'], "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    while text.ends_with(['.', '!', '?']) {
        text.pop();
    }
    let mut lower = text.to_ascii_lowercase();
    for prefix in [
        "please ",
        "can you ",
        "could you ",
        "would you ",
        "will you ",
        "now ",
        "ok ",
        "okay ",
    ] {
        if lower.starts_with(prefix) {
            text = text[prefix.len()..].to_owned();
            lower = text.to_ascii_lowercase();
        }
    }
    if lower.ends_with(" please") {
        text.truncate(text.len() - " please".len());
    }
    text.trim_end_matches(',').trim().to_owned()
}
fn strip_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &text[prefix.len()..])
}
fn strip_any<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|p| strip_ci(text, p))
}
/// Split at the last case-insensitive separator, so "add trip to Paris to my
/// travel list" keeps "trip to Paris" as the item.
fn rsplit_ci<'a>(text: &'a str, separators: &[&str]) -> Option<(&'a str, &'a str)> {
    let lower = text.to_ascii_lowercase();
    separators
        .iter()
        .filter_map(|s| lower.rfind(s).map(|at| (at, s.len())))
        .max_by_key(|(at, _)| *at)
        .map(|(at, len)| (&text[..at], &text[at + len..]))
}
fn split_ci<'a>(text: &'a str, separators: &[&str]) -> Option<(&'a str, &'a str)> {
    let lower = text.to_ascii_lowercase();
    separators
        .iter()
        .filter_map(|s| lower.find(s).map(|at| (at, s.len())))
        .min_by_key(|(at, _)| *at)
        .map(|(at, len)| (&text[..at], &text[at + len..]))
}
fn kind_word(word: &str) -> Option<Kind> {
    match word.to_ascii_lowercase().as_str() {
        "note" => Some(Kind::Note),
        "list" => Some(Kind::List),
        _ => None,
    }
}
/// A spoken reference. Unqualified names are accepted only when a note with
/// that name exists; see [`Outcome::NotMine`].
pub fn target(text: &str) -> Option<Target> {
    let text = text.trim().trim_end_matches(',').trim();
    let lower = text.to_ascii_lowercase();
    match lower.as_str() {
        "it" | "that" | "this" => return Some(Target::That(None)),
        "that list" | "this list" | "the list" => return Some(Target::That(Some(Kind::List))),
        "that note" | "this note" | "the note" => return Some(Target::That(Some(Kind::Note))),
        "" => return None,
        _ => {}
    }
    let named = strip_any(text, &["the ", "my ", "our "])
        .unwrap_or(text)
        .trim();
    // "my list" or "a note" names no particular note: ask, never guess.
    if let Some(kind) = kind_word(named) {
        return Some(Target::That(Some(kind)));
    }
    let (title, kind) = match named.rsplit_once(' ') {
        Some((title, last)) if kind_word(last).is_some() => (title.trim(), kind_word(last)),
        _ => (named, None),
    };
    if title.is_empty() || title.split_whitespace().count() > 12 {
        return None;
    }
    Some(Target::Named {
        title: title.to_owned(),
        kind,
    })
}

/// Creation forms: "create a list called Groceries", "make a shopping list",
/// "start a packing list with socks", "take a note", "take a note saying ...".
/// A missing name is asked for; nothing else is guessed.
///
/// `dictation` is true for "take" and "jot down". Without a name, "make a
/// list of the best films" or "create a note about Rome" asks the companion
/// to write something, so those forms are left to the conversation.
fn creation(rest: &str, dictation: bool) -> Option<Command> {
    let rest = strip_any(
        rest,
        &["me a new ", "me a ", "a new ", "a ", "an ", "my ", "new "],
    )
    .unwrap_or(rest);
    let content_separators = |kind| -> &'static [&'static str] {
        match kind {
            Kind::Note => &[
                " saying ",
                " that says ",
                ": ",
                " with the text ",
                " that ",
                " of ",
                " about ",
            ],
            Kind::List => &[" with ", ": ", " containing ", " of "],
        }
    };
    let content = |text: &str| Some(text.trim().to_owned()).filter(|c| !c.is_empty());
    // An explicit name, split from any content that follows it.
    let named = |kind: Kind, named: &str| -> Command {
        let (title, body) = match split_ci(named, content_separators(kind)) {
            Some((title, body)) => (title, content(body)),
            None => (named, None),
        };
        Command::Create {
            kind,
            title: Some(title.trim().to_owned()),
            content: body,
        }
    };
    // "a list called Groceries ...", "a note titled Plans saying ..."
    if let Some((word, tail)) = rest.split_once(' ')
        && let Some(kind) = kind_word(word)
    {
        if let Some(name) = strip_any(tail, &["called ", "named ", "titled ", "for "]) {
            return Some(named(kind, name));
        }
        // "take a note saying ...", "a list with eggs": ask for the name.
        let lower = tail.to_ascii_lowercase();
        let generative = |sep: &&&str| {
            !dictation
                && matches!(
                    (kind, **sep),
                    (Kind::List, " of ") | (Kind::Note, " about ")
                )
        };
        return content_separators(kind)
            .iter()
            .filter(|sep| !generative(sep))
            .find_map(|sep| strip_ci(&format!(" {lower}"), sep).map(|_| sep.len() - 1))
            .map(|skip| Command::Create {
                kind,
                title: None,
                content: content(&tail[skip..]),
            });
    }
    if let Some(kind) = kind_word(rest) {
        return Some(Command::Create {
            kind,
            title: None,
            content: None,
        });
    }
    // "a shopping list", "a packing list with socks": the spoken words,
    // including "list", are the name. At most five words before the kind.
    for kind in [Kind::List, Kind::Note] {
        let word = format!(" {}", kind.word());
        let lower = rest.to_ascii_lowercase();
        let Some(at) = lower.find(&word) else {
            continue;
        };
        let end = at + word.len();
        let tail = &rest[end..];
        if rest[..end].split_whitespace().count() > 6 {
            return None;
        }
        // "a shopping list called Weekend with ...": the explicit name wins
        // over the spoken words before "list".
        if let Some(name) = naming(tail) {
            return Some(named(kind, name));
        }
        let body = if tail.is_empty() {
            None
        } else {
            match content_separators(kind)
                .iter()
                .find_map(|sep| strip_ci(tail, sep))
            {
                Some(body) => content(body),
                None => continue,
            }
        };
        let title = &rest[..end];
        if title.split_whitespace().count() > 6 {
            return None;
        }
        return Some(Command::Create {
            kind,
            title: Some(title.trim().to_owned()),
            content: body,
        });
    }
    None
}

/// The name after "called", "named" or "titled" that follows the kind word.
fn naming(tail: &str) -> Option<&str> {
    strip_any(tail, &[" called ", " named ", " titled "]).filter(|name| !name.trim().is_empty())
}
/// Recognise a notes request. Returns `None` for ordinary conversation.
pub fn parse(text: &str) -> Option<Command> {
    let text = prepare(text);
    let text = text.as_str();
    let lower = text.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "what notes do i have"
            | "what lists do i have"
            | "what notes and lists do i have"
            | "what are my notes"
            | "what are my lists"
            | "list my notes"
            | "list my lists"
            | "read my notes"
            | "show my notes"
            | "which notes do i have"
    ) {
        return Some(Command::ListAll);
    }
    if let Some(rest) = strip_any(
        text,
        &[
            "find my notes about ",
            "find my note about ",
            "find the note about ",
            "find my list about ",
            "find notes about ",
            "search my notes for ",
            "which note mentions ",
            "which notes mention ",
        ],
    ) {
        let query = rest.trim();
        return (!query.is_empty()).then(|| Command::Find {
            query: query.to_owned(),
        });
    }
    if let Some(rest) = strip_any(
        text,
        &[
            "create ",
            "make ",
            "start ",
            "take ",
            "new ",
            "begin ",
            "jot down ",
        ],
    ) {
        let dictation = strip_any(text, &["take ", "jot down "]).is_some();
        return creation(rest, dictation);
    }
    if let Some(rest) = strip_any(text, &["add ", "append ", "put "]) {
        let (item, place) = rsplit_ci(rest, &[" to ", " onto ", " on "])?;
        return Some(Command::Add {
            target: target(place)?,
            text: item.trim().to_owned(),
        });
    }
    if let Some(rest) = strip_any(
        text,
        &[
            "read me ",
            "read out ",
            "read back ",
            "read ",
            "what's on ",
            "what is on ",
            "what's in ",
            "what is in ",
            "open ",
        ],
    ) {
        let rest = strip_any(rest, &["back "]).unwrap_or(rest);
        let rest = if rest.to_ascii_lowercase().ends_with(" back") {
            &rest[..rest.len() - " back".len()]
        } else {
            rest
        };
        return Some(Command::Read {
            target: target(rest)?,
        });
    }
    for (prefix, done) in [
        ("tick off ", true),
        ("check off ", true),
        ("cross off ", true),
        ("untick ", false),
        ("uncheck ", false),
    ] {
        if let Some(rest) = strip_ci(text, prefix) {
            let (item, place) =
                rsplit_ci(rest, &[" on ", " from ", " in "]).unwrap_or((rest, "that list"));
            return Some(Command::Complete {
                target: target(place)?,
                item: item.trim().to_owned(),
                done,
            });
        }
    }
    if let Some(rest) = strip_ci(text, "mark ") {
        let (body, place) =
            rsplit_ci(rest, &[" on ", " in ", " from "]).unwrap_or((rest, "that list"));
        let lower_body = body.to_ascii_lowercase();
        let (item, done) = [
            (" as not done", false),
            (" as not complete", false),
            (" as done", true),
            (" as complete", true),
            (" as completed", true),
            (" as finished", true),
            (" done", true),
            (" complete", true),
            (" completed", true),
        ]
        .into_iter()
        .find_map(|(suffix, done)| {
            lower_body
                .ends_with(suffix)
                .then(|| (&body[..body.len() - suffix.len()], done))
        })?;
        return Some(Command::Complete {
            target: target(place)?,
            item: item.trim().to_owned(),
            done,
        });
    }
    if let Some(rest) = strip_any(text, &["remove ", "delete ", "take ", "cross "]) {
        if let Some((item, place)) = rsplit_ci(rest, &[" from ", " off "]) {
            return Some(Command::RemoveItem {
                target: target(place)?,
                item: item.trim().to_owned(),
            });
        }
        if strip_ci(text, "take ").is_some() || strip_ci(text, "cross ").is_some() {
            return None;
        }
        let target = target(rest)?;
        // "delete the report" is ordinary conversation; a kind word or an
        // exact focus reference is required for a destructive notes request.
        return match &target {
            Target::Named { kind: None, .. } => None,
            _ => Some(Command::Delete { target }),
        };
    }
    if let Some(rest) = strip_ci(text, "erase ") {
        let target = target(rest)?;
        return match &target {
            Target::Named { kind: None, .. } => None,
            _ => Some(Command::Delete { target }),
        };
    }
    if let Some(rest) = strip_ci(text, "rename ") {
        let (place, title) = rsplit_ci(rest, &[" to ", " as "])?;
        return Some(Command::Rename {
            target: target(place)?,
            title: title.trim().to_owned(),
        });
    }
    if let Some(rest) = strip_any(text, &["change ", "replace ", "rewrite ", "update "]) {
        let (place, body) = split_ci(rest, &[" to say ", " to read ", " to ", " with "])?;
        let target = target(place)?;
        if !matches!(
            target,
            Target::That(Some(Kind::Note))
                | Target::Named {
                    kind: Some(Kind::Note),
                    ..
                }
        ) {
            return None;
        }
        return Some(Command::Replace {
            target,
            text: body.trim().to_owned(),
        });
    }
    None
}

/// The answer to a confirmation read back by [`Session::confirm_question`].
fn confirmation_words(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingAction {
    Delete,
    Replace(String),
}
#[derive(Clone, Debug)]
struct Pending {
    note: String,
    revision: u64,
    action: PendingAction,
    expires: u64,
}
/// Per-person conversational state. Never persisted and never shared.
#[derive(Debug, Default)]
pub struct Session {
    focus: Option<(String, u64)>,
    pending: Option<Pending>,
    draft: Option<(Draft, u64)>,
}
/// A creation or addition waiting for one short spoken answer.
#[derive(Clone, Debug)]
enum Draft {
    Title {
        kind: Kind,
        content: Option<String>,
    },
    Body {
        title: String,
    },
    /// "eggs and milk" without commas: one item or several?
    Split {
        whole: String,
        parts: Vec<String>,
        into: SplitInto,
    },
}
#[derive(Clone, Debug)]
enum SplitInto {
    New(String),
    Existing { id: String, revision: u64 },
}
impl Session {
    pub fn cancel(&mut self) {
        self.focus = None;
        self.pending = None;
        self.draft = None;
    }
    fn ask(&mut self, draft: Draft, now: u64, question: String) -> Outcome {
        self.draft = Some((draft, now + DRAFT_SECONDS));
        Outcome::Reply(question)
    }
    pub fn waiting(&self, now: u64) -> bool {
        self.pending.as_ref().is_some_and(|p| now < p.expires)
    }
    fn focus(&mut self, id: &str, now: u64) {
        self.focus = Some((id.to_owned(), now + FOCUS_SECONDS));
    }
}

/// What a request produced. `changed` notebooks must be durably saved before
/// the reply is spoken; on a save failure the caller discards the change.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not a notes request after all: let the companion answer.
    NotMine,
    Reply(String),
    Changed(Notebook, String),
}

fn resolve(
    book: &Notebook,
    session: &mut Session,
    target: &Target,
    now: u64,
) -> Result<String, Outcome> {
    match target {
        Target::That(kind) => {
            let focused = session
                .focus
                .as_ref()
                .filter(|(_, until)| now < *until)
                .and_then(|(id, _)| book.get(id));
            match focused {
                Some(note) if kind.is_none_or(|k| k == note.kind) => Ok(note.id.clone()),
                Some(note) => Err(Outcome::Reply(format!(
                    "The last one we used was the {} {}. Please say its name.",
                    note.title,
                    note.kind.word()
                ))),
                None => {
                    session.focus = None;
                    Err(Outcome::Reply(format!(
                        "Which {} do you mean? Please say its name.",
                        kind.map_or("note or list", Kind::word)
                    )))
                }
            }
        }
        Target::Named { title, kind } => match book.find(title, *kind) {
            Lookup::Found(id) => Ok(id),
            Lookup::Ambiguous(names) => Err(Outcome::Reply(format!(
                "I found more than one: {}. Which one do you mean?",
                spoken_join(names.iter().map(String::as_str).collect())
            ))),
            Lookup::Missing if kind.is_none() => Err(Outcome::NotMine),
            Lookup::Missing if book.notes.is_empty() => Err(Outcome::Reply(format!(
                "I couldn't find a {} called {title}. You have no notes or lists yet.",
                kind.map_or("note", Kind::word)
            ))),
            Lookup::Missing => Err(Outcome::Reply(format!(
                "I couldn't find a {} called {title}. You have: {}.",
                kind.map_or("note", Kind::word),
                book.titles()
            ))),
        },
    }
}
fn find_item(note: &Note, spoken: &str) -> Result<usize, String> {
    let wanted = normalise(spoken);
    if wanted.is_empty() {
        return Err("Which item do you mean?".into());
    }
    let exact: Vec<_> = (0..note.items.len())
        .filter(|&i| normalise(&note.items[i].text) == wanted)
        .collect();
    let found = if exact.is_empty() {
        let words: Vec<_> = wanted.split(' ').collect();
        (0..note.items.len())
            .filter(|&i| {
                let item = normalise(&note.items[i].text);
                let have: Vec<_> = item.split(' ').collect();
                words.iter().all(|w| have.contains(w))
            })
            .collect()
    } else {
        exact
    };
    match found.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("I couldn't find {spoken} on {}.", note.title)),
        many => Err(format!(
            "More than one item on {} matches: {}. Please say the whole item.",
            note.title,
            spoken_join(many.iter().map(|&i| note.items[i].text.as_str()).collect())
        )),
    }
}
/// Split "milk, eggs and bread" into items. A phrase without commas, such as
/// "salt and pepper", stays one item; the result is always read back.
pub fn split_items(text: &str) -> Vec<String> {
    if !text.contains(',') {
        return vec![text.trim().to_owned()];
    }
    let mut parts: Vec<String> = text.split(',').map(|p| p.trim().to_owned()).collect();
    if let Some(last) = parts.pop() {
        let tail = strip_ci(&last, "and ").unwrap_or(&last);
        match rsplit_ci(tail, &[" and "]) {
            Some((a, b)) => {
                parts.push(a.trim().to_owned());
                parts.push(b.trim().to_owned());
            }
            None => parts.push(tail.trim().to_owned()),
        }
    }
    parts.retain(|p| !p.is_empty());
    parts
}
enum Items {
    Clear(Vec<String>),
    /// "a and b" without commas could be one item or two.
    Unclear(String, Vec<String>),
}
fn list_items(text: &str) -> Items {
    let text = text.trim();
    if text.contains(',') {
        return Items::Clear(split_items(text));
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains(" and ") {
        let mut parts = Vec::new();
        let mut start = 0;
        for (at, _) in lower.match_indices(" and ") {
            parts.push(text[start..at].trim().to_owned());
            start = at + " and ".len();
        }
        parts.push(text[start..].trim().to_owned());
        parts.retain(|p| !p.is_empty());
        if parts.len() > 1 {
            return Items::Unclear(text.to_owned(), parts);
        }
    }
    Items::Clear(vec![text.to_owned()])
}
fn split_question(whole: &str, parts: &[String]) -> String {
    format!(
        "Should I save {whole} as one item, or as {} separate items: {}? Say one item or separate items.",
        parts.len(),
        spoken_join(parts.iter().map(String::as_str).collect())
    )
}
/// "Take a note of that": notes can only keep what is said to them.
fn vague(content: &str) -> bool {
    matches!(
        confirmation_words(content).as_str(),
        "that" | "this" | "it" | "what you said" | "what i said" | "everything"
    )
}
fn strip_title_answer(text: &str) -> &str {
    let text = text.trim();
    strip_any(
        text,
        &[
            "call it ",
            "it's called ",
            "it is called ",
            "name it ",
            "title it ",
            "called ",
            "the title is ",
            "its title is ",
        ],
    )
    .unwrap_or(text)
}

/// An answer to "What should I call it?" that rejects or questions the
/// request instead of naming it. `words` is [`confirmation_words`] output.
fn not_a_name(text: &str, words: &str) -> bool {
    let raw = text.trim().to_ascii_lowercase();
    raw.ends_with('?')
        || ["no,", "no.", "no!", "not "]
            .iter()
            .any(|p| raw.starts_with(p))
        || [
            "nope ",
            "no i ",
            "no that ",
            "no just ",
            "i meant ",
            "i mean ",
            "i didn t ",
            "i did not ",
            "i don t want",
            "i do not want",
            "actually ",
            "that s not ",
            "that is not ",
            "wait ",
            "don t save",
            "do not save",
        ]
        .iter()
        .any(|p| words.starts_with(p) || format!("{words} ") == *p)
}
/// Answer a creation question, or `None` to handle the text as a new request.
fn answer_draft(
    book: &Notebook,
    session: &mut Session,
    draft: Draft,
    text: &str,
    now: u64,
) -> Option<Outcome> {
    let spoken = prepare(text);
    let words = confirmation_words(&spoken);
    if matches!(
        words.as_str(),
        "cancel"
            | "never mind"
            | "nevermind"
            | "no"
            | "stop"
            | "forget it"
            | "nothing"
            | "don t save it"
    ) {
        return Some(Outcome::Reply("Okay, I won't save that.".into()));
    }
    // A different notes request replaces the question.
    if parse(&spoken).is_some() {
        return None;
    }
    // A name is never a correction or a question: "No, I meant tell me some
    // films" or "What are the best films?" drop the question and go to the
    // conversation, so nothing is saved under that wording.
    if matches!(draft, Draft::Title { .. }) && not_a_name(text, &words) {
        return None;
    }
    match draft {
        Draft::Title { kind, content } => {
            let title = strip_title_answer(&spoken);
            let content = content.filter(|c| !vague(c));
            if title.split_whitespace().count() > 8 {
                // Too long to be a name: treat it as a new request.
                return None;
            }
            Some(run(
                book,
                session,
                Command::Create {
                    kind,
                    title: Some(title.to_owned()),
                    content,
                },
                now,
            ))
        }
        Draft::Body { title } => Some(run(
            book,
            session,
            Command::Create {
                kind: Kind::Note,
                title: Some(title),
                content: Some(spoken.clone()),
            },
            now,
        )),
        Draft::Split { whole, parts, into } => {
            let items = match words.as_str() {
                "one item" | "one" | "as one item" | "one thing" | "together"
                | "keep it together" => vec![whole],
                "separate items" | "separately" | "separate" | "as separate items"
                | "split them" | "separate ones" => parts,
                _ => {
                    return Some(Outcome::Reply(
                        "I didn't catch one item or separate items, so nothing was added.".into(),
                    ));
                }
            };
            Some(match into {
                SplitInto::New(title) => create_list(book, session, title, items, now),
                SplitInto::Existing { id, revision } => {
                    if book.revision != revision || book.get(&id).is_none() {
                        Outcome::Reply(
                            "That list changed, so nothing was added. Please say it again.".into(),
                        )
                    } else {
                        add_items(book, session, &id, items, now)
                    }
                }
            })
        }
    }
}
fn next_revision(book: &Notebook) -> Option<Notebook> {
    let mut next = book.clone();
    next.version = 1;
    next.revision = book.revision.checked_add(1).filter(|r| *r < MAX_REVISION)?;
    Some(next)
}
const EXHAUSTED: &str = "Your notes can't take more changes. Export them and start a new notebook.";
fn create_list(
    book: &Notebook,
    session: &mut Session,
    title: String,
    items: Vec<String>,
    now: u64,
) -> Outcome {
    let Some(mut next) = next_revision(book) else {
        return Outcome::Reply(EXHAUSTED.into());
    };
    if book.notes.len() >= MAX_NOTES {
        return Outcome::Reply(
            "You have 200 notes and lists, the most I can keep. Delete one first.".into(),
        );
    }
    if items.len() > MAX_ITEMS {
        return Outcome::Reply("That is more than 100 items.".into());
    }
    let mut note = Note {
        id: new_id(),
        title,
        kind: Kind::List,
        body: String::new(),
        items: Vec::new(),
        created: now,
        updated: now,
    };
    for item in items {
        match clean_text(&item, MAX_ITEM_BYTES) {
            Ok(text) => note.items.push(Item {
                id: new_id(),
                text,
                done: false,
            }),
            Err(e) => return Outcome::Reply(e),
        }
    }
    let reply = match note.items.len() {
        0 => format!("Created a list called {}.", note.title),
        n => format!(
            "Created a list called {} with {n} {}: {}.",
            note.title,
            if n == 1 { "item" } else { "items" },
            spoken_join(note.items.iter().map(|i| i.text.as_str()).collect())
        ),
    };
    session.focus(&note.id, now);
    next.notes.push(note);
    Outcome::Changed(next, reply)
}
fn add_items(
    book: &Notebook,
    session: &mut Session,
    id: &str,
    items: Vec<String>,
    now: u64,
) -> Outcome {
    let Some(mut next) = next_revision(book) else {
        return Outcome::Reply(EXHAUSTED.into());
    };
    let note = next.get_mut(id).expect("resolved note");
    if note.items.len() + items.len() > MAX_ITEMS {
        return Outcome::Reply(format!(
            "{} already has {} items. A list can hold 100.",
            note.title,
            note.items.len()
        ));
    }
    let mut added = Vec::new();
    for item in items {
        match clean_text(&item, MAX_ITEM_BYTES) {
            Ok(text) => {
                added.push(text.clone());
                note.items.push(Item {
                    id: new_id(),
                    text,
                    done: false,
                });
            }
            Err(e) => return Outcome::Reply(e),
        }
    }
    note.updated = now;
    let reply = format!(
        "Added {} to {}.",
        spoken_join(added.iter().map(String::as_str).collect()),
        note.title
    );
    session.focus(id, now);
    Outcome::Changed(next, reply)
}

fn read_note(note: &Note) -> String {
    let mut spoken = match note.kind {
        Kind::Note if note.body.is_empty() => format!("Your {} note is empty.", note.title),
        Kind::Note => format!("{}: {}", note.title, note.body),
        Kind::List if note.items.is_empty() => format!("Your {} list is empty.", note.title),
        Kind::List => {
            let open: Vec<_> = note.items.iter().filter(|i| !i.done).collect();
            let done = note.items.len() - open.len();
            let mut text = format!(
                "{} has {} {}",
                note.title,
                open.len(),
                if open.len() == 1 { "item" } else { "items" }
            );
            if !open.is_empty() {
                text.push_str(": ");
                text.push_str(&spoken_join(open.iter().map(|i| i.text.as_str()).collect()));
            }
            text.push('.');
            if done > 0 {
                text.push_str(&format!(" {done} completed."));
            }
            text
        }
    };
    if spoken.len() > SPOKEN_LIMIT {
        let mut cut = SPOKEN_LIMIT;
        while !spoken.is_char_boundary(cut) {
            cut -= 1;
        }
        spoken.truncate(cut);
        spoken.push_str("… The rest is in Settings, under Notes.");
    }
    spoken
}

/// Apply a confirmation answer or a new request. Every change is computed on a
/// copy; the caller persists `Outcome::Changed` before replacing its notebook.
pub fn handle(book: &Notebook, session: &mut Session, text: &str, now: u64) -> Outcome {
    if let Some((draft, until)) = session.draft.take()
        && now < until
        && let Some(outcome) = answer_draft(book, session, draft, text, now)
    {
        return outcome;
    }
    if let Some(pending) = session.pending.take() {
        let answer = confirmation_words(text);
        let expected = match &pending.action {
            PendingAction::Delete => {
                let kind = book.get(&pending.note).map_or("note", |n| n.kind.word());
                [
                    format!("yes delete that {kind}"),
                    format!("yes please delete that {kind}"),
                ]
            }
            PendingAction::Replace(_) => [
                "yes replace that note".to_owned(),
                "yes please replace that note".to_owned(),
            ],
        };
        if expected.contains(&answer) {
            if now >= pending.expires || pending.revision != book.revision {
                return Outcome::Reply(
                    "That confirmation expired or your notes changed. Nothing was changed.".into(),
                );
            }
            let mut next = book.clone();
            let Some(index) = next.notes.iter().position(|n| n.id == pending.note) else {
                return Outcome::Reply("That note no longer exists. Nothing was changed.".into());
            };
            next.revision =
                match book.revision.checked_add(1).filter(|r| *r < MAX_REVISION) {
                    Some(r) => r,
                    None => return Outcome::Reply(
                        "Your notes can't take more changes. Export them and start a new notebook."
                            .into(),
                    ),
                };
            return match pending.action {
                PendingAction::Delete => {
                    let note = next.notes.remove(index);
                    session.focus = None;
                    Outcome::Changed(
                        next,
                        format!("Deleted the {} {}.", note.title, note.kind.word()),
                    )
                }
                PendingAction::Replace(body) => {
                    let note = &mut next.notes[index];
                    note.body = body;
                    note.updated = now;
                    let title = note.title.clone();
                    session.focus(&pending.note, now);
                    Outcome::Changed(next, format!("Replaced the text of {title}."))
                }
            };
        }
        if matches!(
            answer.as_str(),
            "yes" | "yes please" | "yeah" | "ok" | "okay" | "sure"
        ) && now < pending.expires
        {
            let phrase = expected[0].clone();
            session.pending = Some(pending);
            return Outcome::Reply(format!(
                "To be sure, say {phrase}. Say no to keep it as it is."
            ));
        }
        if matches!(
            answer.as_str(),
            "no" | "cancel" | "no thanks" | "keep it" | "don t" | "do not" | "never mind"
        ) || answer.starts_with("no ")
        {
            return Outcome::Reply("Okay, nothing was changed.".into());
        }
        if answer.starts_with("yes ") && parse(text).is_none() {
            return Outcome::Reply(
                "That didn't match what I asked you to confirm, so nothing was changed.".into(),
            );
        }
        // Any other request revokes the confirmation and is handled normally.
    }
    let Some(command) = parse(text) else {
        return Outcome::NotMine;
    };
    run(book, session, command, now)
}

fn run(book: &Notebook, session: &mut Session, command: Command, now: u64) -> Outcome {
    let mut next = book.clone();
    next.version = 1;
    next.revision = match book.revision.checked_add(1).filter(|r| *r < MAX_REVISION) {
        Some(r) => r,
        None => {
            return Outcome::Reply(
                "Your notes can't take more changes. Export them and start a new notebook.".into(),
            );
        }
    };
    match command {
        Command::ListAll => {
            if book.notes.is_empty() {
                return Outcome::Reply(
                    "You have no notes or lists yet. Say, for example, create a list called groceries.".into(),
                );
            }
            let lists = book.notes.iter().filter(|n| n.kind == Kind::List).count();
            Outcome::Reply(format!(
                "You have {} {} and {} {}: {}.",
                book.notes.len() - lists,
                if book.notes.len() - lists == 1 {
                    "note"
                } else {
                    "notes"
                },
                lists,
                if lists == 1 { "list" } else { "lists" },
                book.titles()
            ))
        }
        Command::Find { query } => {
            let wanted = normalise(&query);
            let words: Vec<_> = wanted.split(' ').filter(|w| !w.is_empty()).collect();
            if words.is_empty() {
                return Outcome::Reply("What should I look for?".into());
            }
            let found: Vec<_> = book
                .notes
                .iter()
                .filter(|n| {
                    let mut text = normalise(&n.title);
                    text.push(' ');
                    text.push_str(&normalise(&n.body));
                    for item in &n.items {
                        text.push(' ');
                        text.push_str(&normalise(&item.text));
                    }
                    let have: Vec<_> = text.split(' ').collect();
                    words.iter().all(|w| have.contains(w))
                })
                .collect();
            match found.as_slice() {
                [] => Outcome::Reply(format!("I didn't find {query} in your notes.")),
                [one] => {
                    session.focus(&one.id, now);
                    Outcome::Reply(format!(
                        "I found your {} {}. Say read that {} to hear it.",
                        one.title,
                        one.kind.word(),
                        one.kind.word()
                    ))
                }
                many => Outcome::Reply(format!(
                    "{} notes mention {query}: {}.",
                    many.len(),
                    spoken_join(many.iter().map(|n| n.title.as_str()).collect())
                )),
            }
        }
        Command::Create {
            kind,
            title,
            content,
        } => {
            let Some(title) = title else {
                let question = if content.as_deref().is_some_and(vague) {
                    format!(
                        "I can only save the words you say to me. What should I call the {}? I'll ask what it should say next.",
                        kind.word()
                    )
                } else {
                    format!("What should I call the {}?", kind.word())
                };
                return session.ask(Draft::Title { kind, content }, now, question);
            };
            let title = match clean_title(&title) {
                Ok(t) => t,
                Err(e) => return Outcome::Reply(e),
            };
            if let Lookup::Found(id) = book.find(&title, Some(kind))
                && book
                    .get(&id)
                    .is_some_and(|n| normalise(&n.title) == normalise(&title))
            {
                session.focus(&id, now);
                return Outcome::Reply(format!(
                    "You already have {}. I'll use that one.",
                    book.get(&id).map(describe).unwrap_or_default()
                ));
            }
            if book.notes.len() >= MAX_NOTES {
                return Outcome::Reply(
                    "You have 200 notes and lists, the most I can keep. Delete one first.".into(),
                );
            }
            if kind == Kind::List {
                return match content.as_deref().map(list_items) {
                    None => create_list(book, session, title, Vec::new(), now),
                    Some(Items::Clear(items)) => create_list(book, session, title, items, now),
                    Some(Items::Unclear(whole, parts)) => {
                        let question = split_question(&whole, &parts);
                        session.ask(
                            Draft::Split {
                                whole,
                                parts,
                                into: SplitInto::New(title),
                            },
                            now,
                            question,
                        )
                    }
                };
            }
            let Some(content) = content else {
                return session.ask(
                    Draft::Body {
                        title: title.clone(),
                    },
                    now,
                    format!("What should the {title} note say?"),
                );
            };
            let body = match clean_text(&content, MAX_BODY_BYTES) {
                Ok(body) => body,
                Err(e) => return Outcome::Reply(e),
            };
            let note = Note {
                id: new_id(),
                title,
                kind,
                body,
                items: Vec::new(),
                created: now,
                updated: now,
            };
            let reply = format!("Saved a note called {}: {}", note.title, note.body);
            session.focus(&note.id, now);
            next.notes.push(note);
            Outcome::Changed(next, reply)
        }
        Command::Read { target } => match resolve(book, session, &target, now) {
            Ok(id) => {
                session.focus(&id, now);
                Outcome::Reply(read_note(book.get(&id).expect("resolved note")))
            }
            Err(outcome) => outcome,
        },
        Command::Add { target, text } => {
            let id = match resolve(book, session, &target, now) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            let note = next.get_mut(&id).expect("resolved note");
            if note.kind == Kind::List {
                return match list_items(&text) {
                    Items::Clear(items) => add_items(book, session, &id, items, now),
                    Items::Unclear(whole, parts) => {
                        let question = split_question(&whole, &parts);
                        session.ask(
                            Draft::Split {
                                whole,
                                parts,
                                into: SplitInto::Existing {
                                    id,
                                    revision: book.revision,
                                },
                            },
                            now,
                            question,
                        )
                    }
                };
            }
            let reply = match note.kind {
                Kind::List => unreachable!("lists return above"),
                Kind::Note => {
                    let text = match clean_text(&text, MAX_BODY_BYTES) {
                        Ok(t) => t,
                        Err(e) => return Outcome::Reply(e),
                    };
                    let joined = if note.body.is_empty() {
                        text.clone()
                    } else {
                        format!("{}\n{text}", note.body)
                    };
                    if joined.len() > MAX_BODY_BYTES {
                        return Outcome::Reply(format!(
                            "{} is full. A note can hold 4,000 characters.",
                            note.title
                        ));
                    }
                    note.body = joined;
                    format!("Added to {}: {text}", note.title)
                }
            };
            note.updated = now;
            session.focus(&id, now);
            Outcome::Changed(next, reply)
        }
        Command::Complete { target, item, done } => {
            if matches!(target, Target::That(_))
                && !session
                    .focus
                    .as_ref()
                    .is_some_and(|(_, until)| now < *until)
                && !item.is_empty()
            {
                // "Mark this as done" without a recent list is not a notes request.
                if session.focus.is_none() {
                    return Outcome::NotMine;
                }
            }
            let id = match resolve(book, session, &target, now) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            let note = next.get_mut(&id).expect("resolved note");
            if note.kind != Kind::List {
                return Outcome::Reply(format!("{} is a note, not a list.", note.title));
            }
            let index = match find_item(note, &item) {
                Ok(i) => i,
                Err(e) => return Outcome::Reply(e),
            };
            let entry = &mut note.items[index];
            session.focus(&id, now);
            if entry.done == done {
                return Outcome::Reply(format!(
                    "{} is already marked {}.",
                    entry.text,
                    if done { "done" } else { "not done" }
                ));
            }
            entry.done = done;
            let reply = format!(
                "Marked {} as {} on {}.",
                entry.text,
                if done { "done" } else { "not done" },
                note.title
            );
            note.updated = now;
            Outcome::Changed(next, reply)
        }
        Command::RemoveItem { target, item } => {
            let id = match resolve(book, session, &target, now) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            let note = next.get_mut(&id).expect("resolved note");
            if note.kind != Kind::List {
                return Outcome::Reply(format!(
                    "{} is a note. To change its text, say change that note to, then the new text.",
                    note.title
                ));
            }
            let index = match find_item(note, &item) {
                Ok(i) => i,
                Err(e) => return Outcome::Reply(e),
            };
            let removed = note.items.remove(index);
            note.updated = now;
            let reply = format!("Removed {} from {}.", removed.text, note.title);
            session.focus(&id, now);
            Outcome::Changed(next, reply)
        }
        Command::Rename { target, title } => {
            let id = match resolve(book, session, &target, now) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            let title = match clean_title(&title) {
                Ok(t) => t,
                Err(e) => return Outcome::Reply(e),
            };
            if book
                .notes
                .iter()
                .any(|n| n.id != id && normalise(&n.title) == normalise(&title))
            {
                return Outcome::Reply(format!("You already have a note or list called {title}."));
            }
            let note = next.get_mut(&id).expect("resolved note");
            let old = std::mem::replace(&mut note.title, title.clone());
            note.updated = now;
            session.focus(&id, now);
            Outcome::Changed(next, format!("Renamed {old} to {title}."))
        }
        Command::Delete { target } => {
            let id = match resolve(book, session, &target, now) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            let note = book.get(&id).expect("resolved note");
            session.pending = Some(Pending {
                note: id.clone(),
                revision: book.revision,
                action: PendingAction::Delete,
                expires: now + CONFIRM_SECONDS,
            });
            let size = match note.kind {
                Kind::List => format!(
                    "{} {}",
                    note.items.len(),
                    if note.items.len() == 1 {
                        "item"
                    } else {
                        "items"
                    }
                ),
                Kind::Note => format!("{} characters", note.body.chars().count()),
            };
            Outcome::Reply(format!(
                "This permanently deletes the {} {} with {size}. Say yes, delete that {} to confirm, or no to keep it.",
                note.title,
                note.kind.word(),
                note.kind.word()
            ))
        }
        Command::Replace { target, text } => {
            let id = match resolve(book, session, &target, now) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            let note = book.get(&id).expect("resolved note");
            if note.kind != Kind::Note {
                return Outcome::Reply(format!(
                    "{} is a list. Add or remove items instead.",
                    note.title
                ));
            }
            let body = match clean_text(&text, MAX_BODY_BYTES) {
                Ok(b) => b,
                Err(e) => return Outcome::Reply(e),
            };
            session.pending = Some(Pending {
                note: id.clone(),
                revision: book.revision,
                action: PendingAction::Replace(body.clone()),
                expires: now + CONFIRM_SECONDS,
            });
            Outcome::Reply(format!(
                "I'll replace the text of {} with: {body}. Say yes, replace that note to confirm, or no to keep it.",
                note.title
            ))
        }
    }
}

/// Plain-language text for a native export. No identifiers or metadata.
pub fn export_text(notes: &[&Note]) -> String {
    let mut out = String::new();
    for note in notes {
        out.push_str(&note.title);
        out.push('\n');
        out.push_str(&"=".repeat(note.title.chars().count().min(80)));
        out.push('\n');
        match note.kind {
            Kind::Note => {
                out.push_str(&note.body);
                out.push('\n');
            }
            Kind::List => {
                for item in &note.items {
                    out.push_str(if item.done { "[x] " } else { "[ ] " });
                    out.push_str(&item.text);
                    out.push('\n');
                }
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(book: &mut Notebook, session: &mut Session, text: &str, now: u64) -> String {
        match handle(book, session, text, now) {
            Outcome::NotMine => panic!("not handled: {text}"),
            Outcome::Reply(reply) => reply,
            Outcome::Changed(next, reply) => {
                next.validate().unwrap();
                *book = next;
                reply
            }
        }
    }

    #[test]
    fn ordinary_conversation_is_left_to_the_companion() {
        let book = Notebook::default();
        let mut s = Session::default();
        for text in [
            "What's the weather like?",
            "Add salt to the soup",
            "Delete the report",
            "Read the news",
            "Remember that I prefer tea",
            "Remind me in ten minutes to stretch",
            "Mark this as done",
            "Take the dog out",
            "Change the model to Terra",
            "I make a list every morning",
            // Generative requests the companion answers; nothing to save.
            "Make a list of the best films of 2020",
            "Can you make a list of ten names for my dog?",
            "Make me a list of pros and cons of remote work",
            "Start a list of my favourite books",
            "Create a note about the Roman empire",
            "Make a new note about the history of tea",
        ] {
            assert_eq!(handle(&book, &mut s, text, 10), Outcome::NotMine, "{text}");
        }
    }

    #[test]
    fn dictate_find_read_append_complete_rename_and_delete_a_list() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        let r = apply(
            &mut book,
            &mut s,
            "Create a list called Groceries with milk, eggs and bread.",
            0,
        );
        assert_eq!(
            r,
            "Created a list called Groceries with 3 items: milk, eggs and bread."
        );
        let r = apply(&mut book, &mut s, "Add oat milk to that list", 10);
        assert_eq!(r, "Added oat milk to Groceries.");
        let r = apply(
            &mut book,
            &mut s,
            "add 2 litres of orange juice to my groceries list",
            20,
        );
        assert_eq!(r, "Added 2 litres of orange juice to Groceries.");
        let r = apply(&mut book, &mut s, "Tick off eggs on the groceries list", 30);
        assert_eq!(r, "Marked eggs as done on Groceries.");
        // "milk" is ambiguous with "oat milk" only by word subset; exact wins.
        let r = apply(&mut book, &mut s, "mark milk as done on that list", 40);
        assert_eq!(r, "Marked milk as done on Groceries.");
        let r = apply(&mut book, &mut s, "What's on my groceries list?", 50);
        assert_eq!(
            r,
            "Groceries has 3 items: bread, oat milk and 2 litres of orange juice. 2 completed."
        );
        let r = apply(&mut book, &mut s, "find my note about orange juice", 60);
        assert!(r.contains("Groceries list"), "{r}");
        let r = apply(&mut book, &mut s, "rename that list to Weekly shop", 70);
        assert_eq!(r, "Renamed Groceries to Weekly shop.");
        let r = apply(
            &mut book,
            &mut s,
            "remove bread from the weekly shop list",
            80,
        );
        assert_eq!(r, "Removed bread from Weekly shop.");
        let r = apply(&mut book, &mut s, "Delete the weekly shop list", 90);
        assert!(r.contains("Say yes, delete that list"), "{r}");
        assert_eq!(book.notes.len(), 1, "delete waits for confirmation");
        let r = apply(&mut book, &mut s, "Yes, delete that list.", 95);
        assert_eq!(r, "Deleted the Weekly shop list.");
        assert!(book.notes.is_empty());
        // Focus was cleared with the deletion: "that list" is not guessed.
        let r = apply(&mut book, &mut s, "add tea to that list", 96);
        assert!(r.starts_with("Which list do you mean?"), "{r}");
    }

    #[test]
    fn notes_keep_source_wording_and_rewrites_need_confirmation() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        let r = apply(
            &mut book,
            &mut s,
            "Take a note called Gate code saying The gate code is 4 7 1 9, ask Priya.",
            0,
        );
        assert_eq!(
            r,
            "Saved a note called Gate code: The gate code is 4 7 1 9, ask Priya"
        );
        apply(&mut book, &mut s, "add Bring the blue key to that note", 5);
        assert_eq!(
            book.notes[0].body,
            "The gate code is 4 7 1 9, ask Priya\nBring the blue key"
        );
        let r = apply(
            &mut book,
            &mut s,
            "change my gate code note to say 5 5 5 1",
            6,
        );
        assert!(
            r.contains("replace the text of Gate code with: 5 5 5 1"),
            "{r}"
        );
        assert!(book.notes[0].body.starts_with("The gate code"));
        // A bare yes or a different phrase does not authorise the change.
        let r = apply(&mut book, &mut s, "yes", 7);
        assert_eq!(
            r,
            "To be sure, say yes replace that note. Say no to keep it as it is."
        );
        assert!(book.notes[0].body.starts_with("The gate code"));
        let r = apply(&mut book, &mut s, "no", 8);
        assert_eq!(r, "Okay, nothing was changed.");
        assert_eq!(
            handle(&book, &mut s, "yes replace that note", 8),
            Outcome::NotMine
        );
        apply(&mut book, &mut s, "change that note to say 5 5 5 1", 9);
        let r = apply(&mut book, &mut s, "Yes, replace that note!", 10);
        assert_eq!(r, "Replaced the text of Gate code.");
        assert_eq!(book.notes[0].body, "5 5 5 1");
        let r = apply(&mut book, &mut s, "read that note", 11);
        assert_eq!(r, "Gate code: 5 5 5 1");
    }

    #[test]
    fn confirmations_are_single_use_expire_and_bind_to_the_revision() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        apply(
            &mut book,
            &mut s,
            "make a note called Plans saying beach",
            0,
        );
        apply(&mut book, &mut s, "make a list called Trip", 1);
        // Expired.
        apply(&mut book, &mut s, "delete my plans note", 2);
        let r = apply(
            &mut book,
            &mut s,
            "yes delete that note",
            2 + CONFIRM_SECONDS,
        );
        assert!(r.contains("expired"), "{r}");
        assert_eq!(book.notes.len(), 2);
        // Replay after use.
        apply(&mut book, &mut s, "delete my plans note", 200);
        apply(&mut book, &mut s, "yes delete that note", 201);
        assert_eq!(book.notes.len(), 1);
        assert_eq!(
            handle(&book, &mut s, "yes delete that note", 202),
            Outcome::NotMine
        );
        // A change to the notebook after the read-back invalidates it.
        apply(&mut book, &mut s, "delete the trip list", 300);
        let stale = book.clone();
        let mut other = Session::default();
        apply(&mut book, &mut other, "add sun cream to the trip list", 301);
        let mut pending = std::mem::take(&mut s);
        let r = apply(&mut book, &mut pending, "yes delete that list", 302);
        assert!(r.contains("changed"), "{r}");
        assert_ne!(stale, book);
        assert_eq!(book.notes.len(), 1);
        // The wrong kind word is not the confirmation for this item.
        apply(&mut book, &mut pending, "delete the trip list", 400);
        let r = apply(&mut book, &mut pending, "yes delete that note", 401);
        assert_eq!(book.notes.len(), 1, "{r}");
        // Cancellation clears pending changes and the follow-up target.
        apply(&mut book, &mut pending, "delete the trip list", 500);
        pending.cancel();
        assert_eq!(
            handle(&book, &mut pending, "yes delete that list", 501),
            Outcome::NotMine
        );
    }

    #[test]
    fn follow_up_references_are_exact_and_expire() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        apply(&mut book, &mut s, "create a list called Hardware", 0);
        apply(
            &mut book,
            &mut s,
            "create a note called Ideas saying owls",
            1,
        );
        // "That list" never falls back to another list when a note is focused.
        let r = apply(&mut book, &mut s, "add screws to that list", 2);
        assert!(r.contains("Ideas note"), "{r}");
        assert!(book.notes[0].items.is_empty());
        apply(&mut book, &mut s, "read the hardware list", 3);
        let r = apply(
            &mut book,
            &mut s,
            "add screws to that list",
            3 + FOCUS_SECONDS,
        );
        assert!(r.starts_with("Which list"), "{r}");
        assert!(book.notes[0].items.is_empty());
    }

    #[test]
    fn ambiguous_and_missing_names_ask_instead_of_guessing() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        apply(&mut book, &mut s, "create a list called Work tasks", 0);
        apply(&mut book, &mut s, "create a list called Home tasks", 1);
        let r = apply(&mut book, &mut s, "add call Sam to the tasks list", 2);
        assert!(r.starts_with("I found more than one"), "{r}");
        assert!(book.notes.iter().all(|n| n.items.is_empty()));
        let r = apply(&mut book, &mut s, "add call Sam to the garden list", 3);
        assert!(r.contains("couldn't find a list called garden"), "{r}");
        assert!(r.contains("Work tasks and Home tasks"), "{r}");
        apply(
            &mut book,
            &mut s,
            "add red apples, green apples to the home tasks list",
            4,
        );
        let r = apply(
            &mut book,
            &mut s,
            "tick off apples on the home tasks list",
            5,
        );
        assert!(r.starts_with("More than one item"), "{r}");
        // A duplicate name is reused, not created twice.
        let r = apply(&mut book, &mut s, "create a list called work Tasks", 6);
        assert!(r.contains("already have"), "{r}");
        assert_eq!(book.notes.len(), 2);
    }

    #[test]
    fn hostile_titles_and_oversized_content_are_rejected_or_bounded() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        for title in [
            "../../etc/passwd",
            "..\\..\\Windows\\System32",
            "/tmp/x",
            "con",
            "<script>alert(1)</script>",
        ] {
            apply(
                &mut book,
                &mut s,
                &format!("create a note called {title} saying x"),
                0,
            );
        }
        // Titles are data, never paths: they are stored as spoken and valid.
        assert_eq!(book.notes.len(), 5);
        book.validate().unwrap();
        let control = format!("create a note called bad{}name", '\u{7}');
        assert!(apply(&mut book, &mut s, &control, 0).contains("can't save"));
        let bidi = "create a note called abc\u{202e}cba";
        assert!(apply(&mut book, &mut s, bidi, 0).contains("can't save"));
        let long = format!("create a note called {}", "x".repeat(81));
        assert!(apply(&mut book, &mut s, &long, 0).contains("too long"));
        let body = format!("create a note called Essay saying {}", "word ".repeat(1000));
        assert!(apply(&mut book, &mut s, &body, 0).contains("too long"));
        assert_eq!(book.notes.len(), 5);
        let mut full = Notebook {
            version: 1,
            ..Default::default()
        };
        for i in 0..MAX_NOTES {
            full.notes.push(Note {
                id: new_id(),
                title: format!("n{i}"),
                kind: Kind::Note,
                body: String::new(),
                items: vec![],
                created: 0,
                updated: 0,
            });
        }
        let r = apply(&mut full, &mut s, "create a note called one more", 0);
        assert!(r.contains("most I can keep"), "{r}");
    }

    #[test]
    fn stored_notebooks_are_validated_before_use() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        apply(&mut book, &mut s, "create a list called Ok with a, b", 0);
        let mut bad = book.clone();
        bad.notes[0].id = "../x".into();
        assert!(bad.validate().is_err());
        let mut bad = book.clone();
        bad.notes.push(bad.notes[0].clone());
        assert!(bad.validate().is_err());
        let mut bad = book.clone();
        bad.notes[0].body = "lists have no body".into();
        assert!(bad.validate().is_err());
        let mut bad = book.clone();
        bad.version = 9;
        assert!(bad.validate().is_err());
        assert!(
            serde_json::from_str::<Notebook>(r#"{"version":1,"revision":0,"notes":[],"extra":1}"#)
                .is_err()
        );
    }

    #[test]
    fn items_split_only_on_commas_and_are_read_back() {
        assert_eq!(split_items("salt and pepper"), vec!["salt and pepper"]);
        assert_eq!(
            split_items("milk, eggs and bread"),
            vec!["milk", "eggs", "bread"]
        );
        assert_eq!(
            split_items("milk, eggs, and bread"),
            vec!["milk", "eggs", "bread"]
        );
        assert_eq!(
            split_items("tea, fish and chips"),
            vec!["tea", "fish", "chips"]
        );
    }

    #[test]
    fn export_is_plain_text_without_identifiers() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        apply(
            &mut book,
            &mut s,
            "create a list called Packing with socks, hat",
            0,
        );
        apply(&mut book, &mut s, "tick off hat on that list", 1);
        let text = export_text(&book.notes.iter().collect::<Vec<_>>());
        assert_eq!(text, "Packing\n=======\n[ ] socks\n[x] hat\n\n");
        assert!(!text.contains(&book.notes[0].id));
    }

    #[test]
    fn natural_creation_asks_only_for_what_is_missing() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        let r = apply(&mut book, &mut s, "Make a shopping list", 0);
        assert_eq!(r, "Created a list called shopping list.");
        let r = apply(&mut book, &mut s, "add oat milk to that list", 1);
        assert_eq!(r, "Added oat milk to shopping list.");
        let r = apply(&mut book, &mut s, "add bread to the shopping list", 2);
        assert_eq!(r, "Added bread to shopping list.");
        // Commas are unambiguous; "and" alone is asked about.
        let r = apply(
            &mut book,
            &mut s,
            "Start a packing list with socks and a charger",
            3,
        );
        assert_eq!(
            r,
            "Should I save socks and a charger as one item, or as 2 separate items: socks and a charger? Say one item or separate items."
        );
        let r = apply(&mut book, &mut s, "Separate items.", 4);
        assert_eq!(
            r,
            "Created a list called packing list with 2 items: socks and a charger."
        );
        let r = apply(&mut book, &mut s, "add salt and pepper to that list", 5);
        assert!(r.starts_with("Should I save salt and pepper"), "{r}");
        let r = apply(&mut book, &mut s, "one item", 6);
        assert_eq!(r, "Added salt and pepper to packing list.");
        // "Take a note" asks for a name, then the words, kept as spoken.
        let r = apply(&mut book, &mut s, "Take a note", 10);
        assert_eq!(r, "What should I call the note?");
        let r = apply(&mut book, &mut s, "Call it Gate code", 11);
        assert_eq!(r, "What should the Gate code note say?");
        let r = apply(&mut book, &mut s, "The code is 4 7 1 9, ask Priya.", 12);
        assert_eq!(
            r,
            "Saved a note called Gate code: The code is 4 7 1 9, ask Priya"
        );
        let r = apply(
            &mut book,
            &mut s,
            "take a note saying the boiler was serviced in May",
            20,
        );
        assert_eq!(r, "What should I call the note?");
        let r = apply(&mut book, &mut s, "Boiler", 21);
        assert_eq!(
            r,
            "Saved a note called Boiler: the boiler was serviced in May"
        );
        let r = apply(&mut book, &mut s, "make a note of that", 30);
        assert!(r.starts_with("I can only save the words you say"), "{r}");
        apply(&mut book, &mut s, "Parking", 31);
        let r = apply(&mut book, &mut s, "Level 3, bay 12", 32);
        assert_eq!(r, "Saved a note called Parking: Level 3, bay 12");
        let r = apply(&mut book, &mut s, "make a list", 40);
        assert_eq!(r, "What should I call the list?");
        assert_eq!(
            apply(&mut book, &mut s, "it's called Chores", 41),
            "Created a list called Chores."
        );
        // Likely notes requests that can't run are explained locally.
        let mut empty = Notebook::default();
        let mut fresh = Session::default();
        let r = apply(&mut empty, &mut fresh, "add milk to my list", 0);
        assert!(r.starts_with("Which list do you mean?"), "{r}");
        let r = apply(&mut empty, &mut fresh, "delete my note", 0);
        assert!(r.starts_with("Which note do you mean?"), "{r}");
    }

    #[test]
    fn dictation_and_named_creation_still_work_after_generative_forms_are_left_alone() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        // Dictation verbs keep "of"/"about" as the words to save.
        assert_eq!(
            apply(&mut book, &mut s, "take a note about the boiler service", 0),
            "What should I call the note?"
        );
        assert_eq!(
            apply(&mut book, &mut s, "Boiler", 1),
            "Saved a note called Boiler: the boiler service"
        );
        assert_eq!(
            apply(&mut book, &mut s, "jot down a list of eggs and milk", 10),
            "What should I call the list?"
        );
        assert!(apply(&mut book, &mut s, "Breakfast", 11).starts_with("Should I save"));
        // A name, or an explicit item separator, is a clear notes request.
        assert_eq!(
            apply(&mut book, &mut s, "make a list called Films", 20),
            "Created a list called Films."
        );
        assert_eq!(
            apply(&mut book, &mut s, "make a list with socks", 30),
            "What should I call the list?"
        );
        assert!(
            apply(&mut book, &mut s, "Packing", 31).starts_with("Created a list called Packing")
        );
    }

    #[test]
    fn an_explicit_name_after_a_compound_kind_is_used() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        // Exact transcript read back from the native Linux voice test.
        assert_eq!(
            apply(
                &mut book,
                &mut s,
                "Make a shopping list called Linux voice test with apples, bread and oat milk.",
                0,
            ),
            "Created a list called Linux voice test with 3 items: apples, bread and oat milk."
        );
        let list = &book.notes[0];
        assert_eq!(list.kind, Kind::List);
        assert_eq!(
            list.items
                .iter()
                .map(|i| i.text.as_str())
                .collect::<Vec<_>>(),
            ["apples", "bread", "oat milk"]
        );
        for (text, reply) in [
            (
                "Make a packing list called Rome trip with socks, sun cream and a hat",
                "Created a list called Rome trip with 3 items: socks, sun cream and a hat.",
            ),
            (
                "start a shopping list named Weekend",
                "Created a list called Weekend.",
            ),
            (
                "create a project note titled Ideas saying owls at dusk",
                "Saved a note called Ideas: owls at dusk",
            ),
        ] {
            assert_eq!(apply(&mut book, &mut s, text, 10), reply, "{text}");
        }
        // Still bounded: a long phrase before "list" is not a notes request.
        assert_eq!(
            handle(
                &book,
                &mut s,
                "make the very long and detailed weekly family shopping list called Big",
                20
            ),
            Outcome::NotMine
        );
    }

    #[test]
    fn a_correction_or_question_never_becomes_a_note_name() {
        for answer in [
            "No, I meant just tell me some films",
            "What are the best films of 2020?",
            "Actually just tell me",
            "I meant give me ideas",
            "no that's not what I wanted",
            "Nope",
            "Don't save anything",
        ] {
            let book = Notebook::default();
            let mut s = Session::default();
            assert_eq!(
                handle(&book, &mut s, "make a list", 0),
                Outcome::Reply("What should I call the list?".into())
            );
            // Not saved, and left to the conversation.
            assert_eq!(
                handle(&book, &mut s, answer, 1),
                Outcome::NotMine,
                "{answer}"
            );
            // The question is gone: a later name is ordinary conversation.
            assert_eq!(
                handle(&book, &mut s, "Films", 2),
                Outcome::NotMine,
                "{answer}"
            );
        }
        // A note's words may still be a question or start with "No".
        let mut book = Notebook::default();
        let mut s = Session::default();
        apply(&mut book, &mut s, "take a note", 0);
        apply(&mut book, &mut s, "Call it Bills", 1);
        assert_eq!(
            apply(&mut book, &mut s, "Did we pay the gas bill?", 2),
            "Saved a note called Bills: Did we pay the gas bill"
        );
        apply(&mut book, &mut s, "take a note", 10);
        apply(&mut book, &mut s, "Parking", 11);
        assert_eq!(
            apply(&mut book, &mut s, "No parking on Tuesdays", 12),
            "Saved a note called Parking: No parking on Tuesdays"
        );
    }

    #[test]
    fn creation_questions_expire_cancel_and_never_swallow_other_requests() {
        let mut book = Notebook::default();
        let mut s = Session::default();
        assert_eq!(
            apply(&mut book, &mut s, "make a list", 0),
            "What should I call the list?"
        );
        // Expired: the answer is ordinary conversation again.
        assert_eq!(
            handle(&book, &mut s, "Groceries", DRAFT_SECONDS),
            Outcome::NotMine
        );
        apply(&mut book, &mut s, "take a note", 100);
        assert_eq!(
            apply(&mut book, &mut s, "never mind", 101),
            "Okay, I won't save that."
        );
        assert_eq!(handle(&book, &mut s, "Groceries", 102), Outcome::NotMine);
        // A long sentence is not taken as a name.
        apply(&mut book, &mut s, "make a list", 200);
        assert_eq!(
            handle(
                &book,
                &mut s,
                "what is the weather going to be like in Leeds tomorrow",
                201
            ),
            Outcome::NotMine
        );
        // A different notes request replaces the question.
        apply(&mut book, &mut s, "make a list", 300);
        let r = apply(&mut book, &mut s, "create a list called Hardware", 301);
        assert_eq!(r, "Created a list called Hardware.");
        // Cancelling (Escape, a speaker change) drops the question.
        apply(&mut book, &mut s, "take a note", 400);
        s.cancel();
        assert_eq!(handle(&book, &mut s, "Secret plans", 401), Outcome::NotMine);
        // An unclear split answer adds nothing; a changed list is not guessed.
        apply(
            &mut book,
            &mut s,
            "add nails and screws to the hardware list",
            500,
        );
        assert!(apply(&mut book, &mut s, "maybe", 501).contains("nothing was added"));
        apply(
            &mut book,
            &mut s,
            "add nails and screws to the hardware list",
            600,
        );
        let stale = book.clone();
        apply(
            &mut book,
            &mut Session::default(),
            "add glue to the hardware list",
            601,
        );
        let r = apply(&mut book, &mut s, "separate items", 602);
        assert!(r.contains("changed"), "{r}");
        assert_ne!(stale, book);
        assert_eq!(book.notes[0].items.len(), 1);
    }
}
