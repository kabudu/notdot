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
    Create {
        kind: Kind,
        title: String,
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
    if let Some(rest) = strip_any(text, &["create ", "make ", "start ", "take ", "new "]) {
        let rest =
            strip_any(rest, &["me a new ", "me a ", "a new ", "a ", "an ", "new "]).unwrap_or(rest);
        let (word, rest) = rest.split_once(' ')?;
        let kind = kind_word(word)?;
        let rest = strip_any(rest, &["called ", "named ", "titled ", "for "])?;
        let separators: &[&str] = match kind {
            Kind::Note => &[" saying ", " that says ", ": ", " with the text "],
            Kind::List => &[" with ", ": ", " containing "],
        };
        let (title, content) = match split_ci(rest, separators) {
            Some((title, content)) => (title, Some(content.trim().to_owned())),
            None => (rest, None),
        };
        return Some(Command::Create {
            kind,
            title: title.trim().to_owned(),
            content: content.filter(|c| !c.is_empty()),
        });
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
}
impl Session {
    pub fn cancel(&mut self) {
        self.focus = None;
        self.pending = None;
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
            let mut note = Note {
                id: new_id(),
                title,
                kind,
                body: String::new(),
                items: Vec::new(),
                created: now,
                updated: now,
            };
            if let Some(content) = content {
                match kind {
                    Kind::Note => match clean_text(&content, MAX_BODY_BYTES) {
                        Ok(body) => note.body = body,
                        Err(e) => return Outcome::Reply(e),
                    },
                    Kind::List => {
                        let items = split_items(&content);
                        if items.len() > MAX_ITEMS {
                            return Outcome::Reply("That is more than 100 items.".into());
                        }
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
                    }
                }
            }
            let reply = match (kind, note.items.len()) {
                (Kind::Note, _) if note.body.is_empty() => format!(
                    "Created a note called {}. Say add, then what to write, to that note.",
                    note.title
                ),
                (Kind::Note, _) => format!("Saved a note called {}: {}", note.title, note.body),
                (Kind::List, 0) => format!("Created a list called {}.", note.title),
                (Kind::List, n) => format!(
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
            let reply = match note.kind {
                Kind::List => {
                    let items = split_items(&text);
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
                    format!(
                        "Added {} to {}.",
                        spoken_join(added.iter().map(String::as_str).collect()),
                        note.title
                    )
                }
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
        apply(&mut book, &mut s, "create a note called Ideas", 1);
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
                &format!("create a note called {title}"),
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
}
