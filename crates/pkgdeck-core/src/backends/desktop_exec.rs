//! A desktop entry's `Exec=` value: reading it into a command and writing
//! one back, following the Desktop Entry Specification's quoting rules.

/// Field codes the launcher fills in (files, URLs, icon, name). Launching
/// without files leaves them out.
const FIELD_CODES: [&str; 13] = [
    "%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v", "%m",
];

/// The `Exec=` value as stored in the file, with the string escapes
/// (`\s`, `\n`, `\t`, `\r`, `\\`) undone.
pub(super) fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split an unescaped `Exec=` value into its arguments. Quoted arguments
/// may contain spaces; inside them `\"`, `` \` ``, `\$` and `\\` stand
/// for the character itself. `None` when a quote is never closed.
pub(super) fn split(exec: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                started = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            escaped @ ('"' | '`' | '$' | '\\') => current.push(escaped),
                            other => {
                                current.push('\\');
                                current.push(other);
                            }
                        },
                        other => current.push(other),
                    }
                }
            }
            ' ' | '\t' => {
                if started {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            other => {
                started = true;
                current.push(other);
            }
        }
    }
    if started {
        arguments.push(current);
    }
    Some(arguments)
}

/// One argument as written in an `Exec=` value: quoted when it holds a
/// character the specification reserves.
pub(super) fn quote(argument: &str) -> String {
    let reserved = |c: char| " \t\n\"'\\><~|&;$*?#()`".contains(c);
    if !argument.is_empty() && !argument.contains(reserved) {
        return argument.to_owned();
    }
    let mut out = String::from("\"");
    for c in argument.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The string escape a desktop entry needs around a whole value.
pub(super) fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(['\n', '\r'], " ")
}

/// What an `Exec=` value runs: environment variables set through a leading
/// `env`, the program, and its arguments.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Command {
    pub environment: Vec<(String, String)>,
    pub program: String,
    pub arguments: Vec<String>,
}

impl Command {
    pub fn parse(exec: &str) -> Option<Self> {
        let mut words = split(&unescape(exec))?.into_iter().peekable();
        let mut environment = Vec::new();
        if words.peek().is_some_and(|word| word == "env") {
            words.next();
            while let Some(pair) = words.next_if(|word| assignment(word).is_some()) {
                environment.push(assignment(&pair)?);
            }
        }
        Some(Self {
            environment,
            program: words.next()?,
            arguments: words.collect(),
        })
    }
    /// Back to an `Exec=` value (string escapes included). The program is
    /// written as given, so callers can keep their own quoting for it.
    pub fn render(&self, program: &str) -> String {
        let mut words = Vec::new();
        if !self.environment.is_empty() {
            words.push("env".to_owned());
            words.extend(
                self.environment
                    .iter()
                    .map(|(name, value)| quote(&format!("{name}={value}")).replace('%', "%%")),
            );
        }
        words.push(program.to_owned());
        words.extend(self.arguments.iter().map(|argument| quote(argument)));
        escape(&words.join(" "))
    }
    /// The arguments to start it with when no files are given: field codes
    /// dropped, `%%` back to `%`.
    pub fn launch_arguments(&self) -> Vec<String> {
        self.arguments
            .iter()
            .filter(|argument| !FIELD_CODES.contains(&argument.as_str()))
            .map(|argument| argument.replace("%%", "%"))
            .collect()
    }
}

/// `NAME=value` with a valid variable name.
pub(super) fn assignment(word: &str) -> Option<(String, String)> {
    let (name, value) = word.split_once('=')?;
    valid_name(name).then(|| (name.to_owned(), value.to_owned()))
}

/// A portable environment variable name.
pub(super) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_and_plain_arguments() {
        assert_eq!(
            split(r#""/opt/My App/app" --name "a \"b\" \$c \\d" plain"#).unwrap(),
            ["/opt/My App/app", "--name", r#"a "b" $c \d"#, "plain"]
        );
        assert_eq!(split("  a\t b  ").unwrap(), ["a", "b"]);
        assert_eq!(split(r#""" x"#).unwrap(), ["", "x"]);
        assert_eq!(split(r#""\q""#).unwrap(), [r"\q"]);
        assert_eq!(split(r#""unclosed"#), None);
        assert_eq!(split(r#""trailing\"#), None);
    }

    #[test]
    fn unescapes_string_escapes() {
        assert_eq!(unescape(r"a\sb\tc\nd\re\\f\;g\"), "a b\tc\nd\re\\f\\;g\\");
    }

    #[test]
    fn quotes_only_what_needs_it() {
        assert_eq!(quote("--no-sandbox"), "--no-sandbox");
        assert_eq!(quote("%U"), "%U");
        assert_eq!(quote(""), r#""""#);
        assert_eq!(quote(r#"a b"c$d`e\f"#), r#""a b\"c\$d\`e\\f""#);
    }

    #[test]
    fn commands_round_trip_with_environment() {
        let exec =
            r#"env DESKTOPINTEGRATION=1 "NAME=a b" "/data/pkgdeck-1.AppImage" --no-sandbox %U"#;
        let command = Command::parse(&escape(exec)).unwrap();
        assert_eq!(
            command.environment,
            [
                ("DESKTOPINTEGRATION".to_owned(), "1".to_owned()),
                ("NAME".to_owned(), "a b".to_owned())
            ]
        );
        assert_eq!(command.program, "/data/pkgdeck-1.AppImage");
        assert_eq!(command.arguments, ["--no-sandbox", "%U"]);
        assert_eq!(command.launch_arguments(), ["--no-sandbox"]);
        assert_eq!(
            Command::parse(&command.render(r#""/data/pkgdeck-1.AppImage""#)).unwrap(),
            command
        );
        // A percent sign in a value is written doubled, and launched single.
        let percent = Command {
            environment: vec![("RATE".into(), "50%".into())],
            program: "/app".into(),
            arguments: vec!["--rate=50%%".into()],
        };
        assert_eq!(percent.render("/app"), r#"env RATE=50%% /app --rate=50%%"#);
        assert_eq!(percent.launch_arguments(), ["--rate=50%"]);
        // `env` without assignments, and no program at all.
        assert_eq!(Command::parse("env /app").unwrap().program, "/app");
        assert_eq!(Command::parse("env A=1"), None);
        assert_eq!(Command::parse(""), None);
        assert_eq!(Command::parse(r#""open"#), None);
    }

    #[test]
    fn variable_names_are_portable() {
        assert!(valid_name("_A1"));
        assert!(!valid_name("1A"));
        assert!(!valid_name("A-B"));
        assert!(!valid_name(""));
        assert_eq!(assignment("A=b=c"), Some(("A".into(), "b=c".into())));
        assert_eq!(assignment("--flag"), None);
    }
}
