//! Two-set Hangul composition: the preedit/commit states an input method goes through while the
//! text is typed one jamo at a time, so that pages see real composition events.

const CONSONANTS: &str = "ㄱㄲㄳㄴㄵㄶㄷㄸㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅃㅄㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ";
const VOWELS: &str = "ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ";
const INITIALS: &str = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ";
const FINALS: &str = "ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ";
const COMPOUND_VOWELS: [(char, char, char); 7] = [
    ('ㅘ', 'ㅗ', 'ㅏ'),
    ('ㅙ', 'ㅗ', 'ㅐ'),
    ('ㅚ', 'ㅗ', 'ㅣ'),
    ('ㅝ', 'ㅜ', 'ㅓ'),
    ('ㅞ', 'ㅜ', 'ㅔ'),
    ('ㅟ', 'ㅜ', 'ㅣ'),
    ('ㅢ', 'ㅡ', 'ㅣ'),
];
const COMPOUND_FINALS: [(char, char, char); 11] = [
    ('ㄳ', 'ㄱ', 'ㅅ'),
    ('ㄵ', 'ㄴ', 'ㅈ'),
    ('ㄶ', 'ㄴ', 'ㅎ'),
    ('ㄺ', 'ㄹ', 'ㄱ'),
    ('ㄻ', 'ㄹ', 'ㅁ'),
    ('ㄼ', 'ㄹ', 'ㅂ'),
    ('ㄽ', 'ㄹ', 'ㅅ'),
    ('ㄾ', 'ㄹ', 'ㅌ'),
    ('ㄿ', 'ㄹ', 'ㅍ'),
    ('ㅀ', 'ㄹ', 'ㅎ'),
    ('ㅄ', 'ㅂ', 'ㅅ'),
];
const SYLLABLES: std::ops::RangeInclusive<u32> = 0xAC00..=0xD7A3;

/// Whether `c` is a Hangul syllable or compatibility jamo, i.e. something an input method composes.
pub(crate) fn is_hangul(c: char) -> bool {
    keystrokes(c).is_some()
}

fn index(set: &str, jamo: char) -> Option<u32> {
    set.chars().position(|c| c == jamo).map(|i| i as u32)
}

fn is_consonant(c: char) -> bool {
    CONSONANTS.contains(c)
}

fn is_vowel(c: char) -> bool {
    VOWELS.contains(c)
}

/// The keystrokes (basic jamo) that type `c`, or `None` when `c` is not Hangul.
fn keystrokes(c: char) -> Option<Vec<char>> {
    let split = |jamo: char| -> Vec<char> {
        COMPOUND_VOWELS
            .iter()
            .chain(COMPOUND_FINALS.iter())
            .find(|(whole, ..)| *whole == jamo)
            .map_or_else(|| vec![jamo], |&(_, first, second)| vec![first, second])
    };
    if SYLLABLES.contains(&u32::from(c)) {
        let offset = u32::from(c) - 0xAC00;
        let initial = INITIALS.chars().nth((offset / 588) as usize)?;
        let vowel = VOWELS.chars().nth((offset % 588 / 28) as usize)?;
        let mut keys = vec![initial];
        keys.extend(split(vowel));
        if offset % 28 != 0 {
            keys.extend(split(FINALS.chars().nth((offset % 28 - 1) as usize)?));
        }
        Some(keys)
    } else if is_consonant(c) || is_vowel(c) {
        Some(split(c))
    } else {
        None
    }
}

/// The syllable (or lone jamo) a preedit shows.
#[derive(Clone, Copy, Default)]
struct Preedit {
    initial: Option<char>,
    vowel: Option<char>,
    last: Option<char>,
}

impl Preedit {
    fn is_empty(&self) -> bool {
        self.initial.is_none() && self.vowel.is_none()
    }

    fn text(&self) -> String {
        match (self.initial, self.vowel) {
            (Some(initial), Some(vowel)) => {
                let code = 0xAC00
                    + (index(INITIALS, initial).unwrap_or(0) * 21
                        + index(VOWELS, vowel).unwrap_or(0))
                        * 28
                    + self
                        .last
                        .and_then(|last| index(FINALS, last))
                        .map_or(0, |i| i + 1);
                char::from_u32(code).map(String::from).unwrap_or_default()
            }
            (Some(jamo), None) | (None, Some(jamo)) => jamo.to_string(),
            (None, None) => String::new(),
        }
    }
}

fn combine(table: &[(char, char, char)], first: char, second: char) -> Option<char> {
    table
        .iter()
        .find(|(_, a, b)| *a == first && *b == second)
        .map(|&(whole, ..)| whole)
}

/// One input-method update: text to commit (replacing the current preedit), then the new
/// preedit, which may be empty.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Update {
    pub(crate) commit: String,
    pub(crate) preedit: String,
}

/// Updates, one per keystroke, that type `text`, ending with a commit of the last preedit. The
/// committed text, joined, equals `text`; `None` when composing it would produce something else
/// (a lone jamo that merges into the previous syllable), so the caller inserts it unchanged.
pub(crate) fn compose(text: &str) -> Option<Vec<Update>> {
    let mut state = Preedit::default();
    let mut updates = Vec::new();
    for c in text.chars() {
        let Some(keys) = keystrokes(c) else {
            let commit = format!("{}{c}", state.text());
            state = Preedit::default();
            updates.push(Update {
                commit,
                preedit: String::new(),
            });
            continue;
        };
        for key in keys {
            let mut commit = String::new();
            let mut next = state;
            if is_consonant(key) {
                match (state.initial, state.vowel, state.last) {
                    (Some(_), Some(_), None) if index(FINALS, key).is_some() => {
                        next.last = Some(key);
                    }
                    (Some(_), Some(_), Some(last))
                        if combine(&COMPOUND_FINALS, last, key).is_some() =>
                    {
                        next.last = combine(&COMPOUND_FINALS, last, key);
                    }
                    _ => {
                        commit = state.text();
                        next = Preedit {
                            initial: Some(key),
                            ..Preedit::default()
                        };
                    }
                }
            } else {
                match (state.initial, state.vowel, state.last) {
                    (None, None, _) => next.vowel = Some(key),
                    (Some(_), None, _) => next.vowel = Some(key),
                    (_, Some(vowel), None) => {
                        if let Some(whole) = combine(&COMPOUND_VOWELS, vowel, key) {
                            next.vowel = Some(whole);
                        } else {
                            commit = state.text();
                            next = Preedit {
                                vowel: Some(key),
                                ..Preedit::default()
                            };
                        }
                    }
                    (Some(initial), Some(vowel), Some(last)) => {
                        let split = COMPOUND_FINALS.iter().find(|(whole, ..)| *whole == last);
                        let (kept, moved) = match split {
                            Some(&(_, first, second)) => (Some(first), second),
                            None => (None, last),
                        };
                        commit = Preedit {
                            initial: Some(initial),
                            vowel: Some(vowel),
                            last: kept,
                        }
                        .text();
                        next = Preedit {
                            initial: Some(moved),
                            vowel: Some(key),
                            last: None,
                        };
                    }
                    (None, Some(_), Some(_)) => {}
                }
            }
            state = next;
            updates.push(Update {
                commit,
                preedit: state.text(),
            });
        }
    }
    if !state.is_empty() {
        updates.push(Update {
            commit: state.text(),
            preedit: String::new(),
        });
    }
    let typed: String = updates
        .iter()
        .map(|update| update.commit.as_str())
        .collect();
    (typed == text).then_some(updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preedits(text: &str) -> Vec<(String, String)> {
        compose(text)
            .unwrap()
            .into_iter()
            .map(|update| (update.commit, update.preedit))
            .collect()
    }

    #[test]
    fn a_syllable_grows_one_jamo_at_a_time() {
        let steps = preedits("한");
        let shown: Vec<&str> = steps.iter().map(|(_, preedit)| preedit.as_str()).collect();
        assert_eq!(shown, ["ㅎ", "하", "한", ""]);
        assert_eq!(steps.last().unwrap().0, "한");
    }

    #[test]
    fn a_final_consonant_moves_to_the_next_syllable() {
        let steps = preedits("하나");
        // ㅎ ㅏ ㄴ ㅏ: the vowel pulls ㄴ out of 한 into 나.
        assert_eq!(steps[3], ("하".into(), "나".into()));
    }

    #[test]
    fn compound_vowels_and_finals_go_through_their_halves() {
        let shown: Vec<String> = preedits("괜닭").into_iter().map(|(_, p)| p).collect();
        assert!(shown.contains(&"고".to_owned()), "{shown:?}");
        assert!(shown.contains(&"괘".to_owned()), "{shown:?}");
        assert!(shown.contains(&"달".to_owned()), "{shown:?}");
        assert!(shown.contains(&"닭".to_owned()), "{shown:?}");
    }

    #[test]
    fn other_characters_commit_the_preedit() {
        let steps = preedits("안 a");
        assert!(steps.iter().any(|(commit, _)| commit == "안 "), "{steps:?}");
        let joined: String = steps.iter().map(|(commit, _)| commit.as_str()).collect();
        assert_eq!(joined, "안 a");
    }

    #[test]
    fn text_that_would_merge_is_left_to_plain_insertion() {
        assert!(compose("갈ㅅ").is_none());
        assert!(compose("안녕하세요 닭을 읽어").is_some());
    }
}
