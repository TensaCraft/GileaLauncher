//! Command-line words as the player types them: a line splits at blanks, quotes keep blanks inside
//! one word. `"` quotes anywhere; `'` quotes at a word's start or right after a closing quote, and
//! is a letter elsewhere (`O'Brien`). Backslashes are plain characters (Windows paths).

/// The official Minecraft launcher's garbage-collector settings: G1 tuned for the game, valid on
/// every Java the game runs on (8 to 25). A build gets them unless it sets or tunes its collector.
pub const DEFAULT_GC_ARGUMENTS: [&str; 6] = [
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:+UseG1GC",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=50",
    "-XX:G1HeapRegionSize=32M",
];

/// The words of `line`.
pub fn words(line: &str) -> Vec<String> {
    let (mut words, mut word, mut started, mut quote) = (Vec::new(), String::new(), false, None);
    let mut closed = false;
    for c in line.chars() {
        let was_closed = std::mem::take(&mut closed);
        match quote {
            Some(open) if c == open => (quote, closed) = (None, true),
            Some(_) => word.push(c),
            None if c == '"' || (c == '\'' && (!started || was_closed)) => (quote, started) = (Some(c), true),
            None if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None => (started, _) = (true, word.push(c)),
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// `word` as a line that `words` reads back as that one word: as it is when it has no blanks or
/// quotes, else in double quotes (a `"` inside closes them, comes as `'"'`, and opens them again).
pub fn shown(word: &str) -> String {
    let plain = !word.is_empty() && !word.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'');
    if plain {
        return word.to_string();
    }
    let mut line = String::from('"');
    for c in word.chars() {
        if c == '"' { line.push_str(r#""'"'""#) } else { line.push(c) }
    }
    line.push('"');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_splits_at_blanks() {
        assert_eq!(words("  -Xmx4G\t-XX:+UseG1GC  -Dx=1 "), ["-Xmx4G", "-XX:+UseG1GC", "-Dx=1"]);
        assert!(words("   ").is_empty());
    }

    #[test]
    fn quotes_keep_blanks_in_one_word() {
        assert_eq!(
            words(r#"-Dname="a b" '-Dpath=C:\My Games\x' -Dq=it"s""#),
            ["-Dname=a b", r"-Dpath=C:\My Games\x", "-Dq=its"]
        );
        assert_eq!(words(r#"-Dempty="" x"#), ["-Dempty=", "x"]);
        assert_eq!(words(r#""unclosed quote"#), ["unclosed quote"], "an open quote runs to the end");
    }

    #[test]
    fn an_apostrophe_inside_a_word_is_a_letter() {
        assert_eq!(
            words(r"-Djava.io.tmpdir=C:\Users\O'Brien\tmp -Xss4M"),
            [r"-Djava.io.tmpdir=C:\Users\O'Brien\tmp", "-Xss4M"]
        );
        assert_eq!(words("'a b' it's"), ["a b", "it's"], "at a word's start it still quotes");
    }

    #[test]
    fn shown_words_read_back_as_themselves() {
        for word in ["-Xmx4G", "-Dname=a b", r"C:\My Games\x", r#"say "hi""#, "it's", r#"both " and '"#] {
            let line = shown(word);
            assert_eq!(words(&line), [word], "{line}");
        }
        assert_eq!(shown("-Xmx4G"), "-Xmx4G", "a plain word stays plain");
    }
}
