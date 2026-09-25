//! Message templates: `{match}` and named-capture placeholders with
//! `{{` / `}}` escapes, parsed and validated at config-load time.

/// One piece of a parsed [`Template`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// Literal text, copied through verbatim.
    Text(String),
    /// A placeholder: `match` for the whole match, or a named capture group.
    Placeholder(String),
}

/// A rule message with placeholders resolved per match.
///
/// Syntax:
///
/// * `{match}` — the text the rule matched.
/// * `{name}` — the text of the named capture group `name`.
/// * `{{` and `}}` — literal braces.
///
/// [`Template::parse`] rejects malformed placeholders, and
/// [`Template::validate`] rejects names that the rule's regex has no
/// capture group for — both at config-load time, never mid-lint.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Template(Vec<Part>);

impl Template {
    /// Parses a template, rejecting malformed placeholders.
    ///
    /// A `{` must open a placeholder (`{match}`, `{name}`) or escape itself
    /// (`{{`); a lone `}` is literal text.
    pub fn parse(src: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut chars = src.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' => match chars.peek() {
                    Some('{') => {
                        chars.next();
                        text.push('{');
                    }
                    Some(&next) if next == '}' || next.is_ascii_alphanumeric() || next == '_' => {
                        let mut name = String::new();
                        loop {
                            match chars.peek() {
                                Some('}') => {
                                    chars.next();
                                    break;
                                }
                                Some(&n)
                                    if name.is_empty()
                                        && (n.is_ascii_alphabetic() || n == '_') =>
                                {
                                    name.push(n);
                                    chars.next();
                                }
                                Some(&n) if !name.is_empty() && n.is_ascii_alphanumeric() => {
                                    name.push(n);
                                    chars.next();
                                }
                                _ => break,
                            }
                        }
                        if name.is_empty() {
                            return Err(
                                "expected a placeholder name after `{` (e.g. `{match}`), or \
                                 `{{` for a literal brace"
                                    .into(),
                            );
                        }
                        if !text.is_empty() {
                            parts.push(Part::Text(std::mem::take(&mut text)));
                        }
                        parts.push(Part::Placeholder(name));
                    }
                    Some(_) => {
                        return Err(
                            "expected a placeholder name after `{` (e.g. `{match}`), or \
                             `{{` for a literal brace"
                                .into(),
                        )
                    }
                    None => return Err("unclosed `{` at the end of the template".into()),
                },
                '}' => {
                    if chars.peek() == Some(&'}') {
                        chars.next();
                    }
                    text.push('}');
                }
                _ => text.push(c),
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(Self(parts))
    }

    /// Checks every placeholder against `regex`'s capture names.
    pub fn validate(&self, regex: &regex::Regex) -> Result<(), String> {
        for part in &self.0 {
            if let Part::Placeholder(name) = part {
                if name != "match" && !regex.capture_names().flatten().any(|n| n == name) {
                    return Err(format!(
                        "placeholder `{{{name}}}` does not name a capture group of the rule's \
                         pattern (known groups: {})",
                        known_groups(regex),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Renders the template for one match.
    pub fn render(&self, caps: &regex::Captures<'_>) -> String {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Placeholder(name) => {
                    let replacement = if name == "match" {
                        caps.get(0).map(|m| m.as_str())
                    } else {
                        caps.name(name).map(|m| m.as_str())
                    };
                    if let Some(replacement) = replacement {
                        out.push_str(replacement);
                    }
                }
            }
        }
        out
    }
}

fn known_groups(regex: &regex::Regex) -> String {
    let names: Vec<&str> = regex.capture_names().flatten().collect();
    if names.is_empty() {
        "(none — name a group with `(?<name>...)`)".to_string()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(src: &str, pattern: &str, haystack: &str) -> String {
        let template = Template::parse(src).unwrap();
        let regex = regex::Regex::new(pattern).unwrap();
        template.validate(&regex).unwrap();
        template
            .render(&regex.captures(haystack).expect("must match"))
    }

    #[test]
    fn literal_passes_through() {
        assert_eq!(render("no placeholders here", "x", "x"), "no placeholders here");
    }

    #[test]
    fn match_placeholder() {
        assert_eq!(render("found '{match}'", "o+", "foo"), "found 'oo'");
    }

    #[test]
    fn named_group_placeholder() {
        assert_eq!(
            render("got {word}!", "(?<word>\\w+)", "hi"),
            "got hi!"
        );
    }

    #[test]
    fn brace_escapes() {
        assert_eq!(render("{{literal}} {match}", "x", "x"), "{literal} x");
    }

    #[test]
    fn lone_closing_brace_is_literal() {
        assert_eq!(render("a } b", "x", "x"), "a } b");
    }

    #[test]
    fn unknown_placeholder_rejected() {
        let template = Template::parse("{nope}").unwrap();
        let regex = regex::Regex::new("x").unwrap();
        assert!(template.validate(&regex).is_err());
    }

    #[test]
    fn malformed_placeholders_rejected() {
        for src in ["{", "{1x}", "{ bad}", "o{" ] {
            assert!(Template::parse(src).is_err(), "{src:?} must not parse");
        }
    }
}
