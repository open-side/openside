//! Minimal glob matcher: `*` matches any run of characters except `/`,
//! `**` matches any run including `/`, `?` matches one non-`/` character.
//! Everything else is literal. No character classes, no escapes.
//!
//! Matching is a set-of-positions simulation, O(pattern × text): patterns come from
//! `PUT /v1/policy`, so backtracking over repeated wildcards must not be able to stall a
//! policy check.

/// Returns true when `text` matches `pattern`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // reachable[k]: the pattern consumed so far can end right before t[k].
    let mut reachable = vec![false; t.len() + 1];
    reachable[0] = true;
    let mut next = vec![false; t.len() + 1];
    let mut pi = 0;
    while pi < p.len() {
        next.fill(false);
        match p[pi] {
            '*' => {
                let double = pi + 1 < p.len() && p[pi + 1] == '*';
                pi += if double { 2 } else { 1 };
                for k in 0..=t.len() {
                    next[k] = reachable[k] || (k > 0 && next[k - 1] && (double || t[k - 1] != '/'));
                }
            }
            c => {
                pi += 1;
                for k in 0..t.len() {
                    next[k + 1] = reachable[k] && if c == '?' { t[k] != '/' } else { t[k] == c };
                }
            }
        }
        std::mem::swap(&mut reachable, &mut next);
        if !reachable.contains(&true) {
            return false;
        }
    }
    reachable[t.len()]
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn literal_and_wildcards() {
        assert!(glob_match("abc", "abc"));
        assert!(!glob_match("abc", "abcd"));
        assert!(glob_match("a*c", "abbbc"));
        assert!(!glob_match("a*c", "ab/c"));
        assert!(glob_match("a**c", "ab/b/c"));
        assert!(glob_match("/work/**", "/work/a/b/c.txt"));
        assert!(glob_match("/work/**", "/work/"));
        assert!(!glob_match("/work/*", "/work/a/b"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "a/c"));
        assert!(glob_match("**", ""));
        assert!(glob_match("*", ""));
        assert!(glob_match("**", "anything/at/all"));
        assert!(glob_match("***", "a/b"));
        assert!(!glob_match("", "a"));
        assert!(glob_match("", ""));
    }

    /// G10: a pattern that makes a backtracking matcher exponential finishes instantly.
    #[test]
    fn adversarial_patterns_match_in_linear_passes() {
        let pattern = "*a".repeat(500) + "b";
        let text = "a".repeat(1024);
        let started = std::time::Instant::now();
        assert!(!glob_match(&pattern, &text));
        assert!(!glob_match(&("**a".repeat(340) + "b"), &"a".repeat(1023)));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
