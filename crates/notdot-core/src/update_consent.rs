//! A voice transcript never grants update permission through model interpretation.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Install,
    Later,
    Unknown,
}
pub fn parse(text: &str) -> Answer {
    let normalized = text.trim().trim_end_matches(['.', '!', '?']).to_lowercase();
    match normalized.as_str() {
        "yes" | "yes please" | "install" | "install update" | "install the update"
        | "update now" => Answer::Install,
        "no" | "no thanks" | "not now" | "later" | "cancel" => Answer::Later,
        _ => Answer::Unknown,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_answers_grant_installation() {
        assert_eq!(parse(" Yes please. "), Answer::Install);
        assert_eq!(parse("Later"), Answer::Later);
        for text in [
            "don't install",
            "yes but not now",
            "yesterday",
            "",
            "install malware",
            "I said yes earlier",
        ] {
            assert_eq!(parse(text), Answer::Unknown);
        }
    }
}
