//! The guard: decides what kind of statement a piece of SQL is, so that
//! read-only mode can refuse everything that is not a plain read.
//! (SAFE-002, SAFE-003)
//!
//! This module is pure: text in, verdict out. It never touches a database.
//!
//! It is deliberately not a SQL parser. It is a small lexer that understands
//! just enough (comments, strings, quoted names) to find the real keywords,
//! plus a classifier that looks at those keywords.
//!
//! Two rules shape every choice in here:
//!
//! 1. **Fail closed.** Anything the guard does not recognise is `Unknown`,
//!    and `Unknown` never runs in read-only mode. A wrongly refused query is
//!    an annoyance; a wrongly allowed one is an incident.
//! 2. **Refuse what dialects disagree on.** Databases differ on how strings
//!    and comments end (backslash escapes, nested comments, `[x]`, `$$`).
//!    If two databases could read the same text differently, an attacker can
//!    hide a statement in the difference, so the guard refuses the text
//!    rather than guessing.
//!
//! The guard is one layer. Backends add their own (a read-only connection),
//! and least-privilege credentials are the real guarantee.

/// What a piece of SQL would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Only reads data. The one kind allowed in read-only mode.
    Read,
    /// Changes rows.
    Write,
    /// Changes structure or permissions.
    Ddl,
    /// Changes the session, a transaction, or database settings.
    Session,
    /// More than one statement was given.
    Multiple,
    /// Nothing but whitespace and comments.
    Empty,
    /// Not recognised, or ambiguous. Treated as unsafe.
    Unknown,
}

/// The guard's answer for one input, with a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub kind: Kind,
    pub reason: String,
}

impl Verdict {
    fn new(kind: Kind, reason: impl Into<String>) -> Self {
        Verdict {
            kind,
            reason: reason.into(),
        }
    }

    /// Read-only mode runs reads and nothing else.
    pub fn allowed_read_only(&self) -> bool {
        self.kind == Kind::Read
    }
}

/// Classify one input. This is the only entry point.
pub fn classify(sql: &str) -> Verdict {
    let tokens = match lex(sql) {
        Ok(tokens) => tokens,
        Err(reason) => return Verdict::new(Kind::Unknown, reason),
    };

    // Split on semicolons and ignore empty pieces, so a trailing `;` is fine.
    let statements: Vec<&[Token]> = tokens
        .split(|t| *t == Token::Semicolon)
        .filter(|s| !s.is_empty())
        .collect();

    match statements.as_slice() {
        [] => Verdict::new(Kind::Empty, "nothing to run"),
        [only] => classify_statement(only),
        _ => Verdict::new(Kind::Multiple, "only one statement may be run at a time"),
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A bare word (keyword or unquoted name), upper-cased.
    Word(String),
    Semicolon,
    OpenParen,
    /// Anything else: strings, quoted names, numbers, operators. The
    /// classifier never needs to look inside these.
    Other,
}

/// Turn SQL text into tokens, dropping comments and whitespace.
/// An `Err` carries the reason the text is refused outright.
fn lex(sql: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();

        if c.is_whitespace() {
            i += 1;
        } else if c.is_control() {
            return Err("contains a control character".into());
        } else if c == '-' && next == Some('-') {
            // Line comment: skip to end of line.
            // Known gap for a future MySQL backend: MySQL only treats `--`
            // as a comment when whitespace follows. Handle when it is added.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i = skip_block_comment(&chars, i)?;
        } else if c == '#' {
            return Err("'#' is a comment in some databases and an operator in others".into());
        } else if c == '\'' || c == '"' || c == '`' {
            i = skip_quoted(&chars, i)?;
            tokens.push(Token::Other);
        } else if c == '[' {
            i = skip_bracketed(&chars, i)?;
            tokens.push(Token::Other);
        } else if c == '$' {
            i = skip_dollar(&chars, i)?;
            tokens.push(Token::Other);
        } else if c == ';' {
            tokens.push(Token::Semicolon);
            i += 1;
        } else if c == '(' {
            tokens.push(Token::OpenParen);
            i += 1;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            tokens.push(Token::Word(word.to_uppercase()));
        } else {
            tokens.push(Token::Other);
            i += 1;
        }
    }
    Ok(tokens)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Skip a `/* ... */` comment starting at `start`. Returns the index after it.
fn skip_block_comment(chars: &[char], start: usize) -> Result<usize, String> {
    if chars.get(start + 2) == Some(&'!') {
        // MySQL runs the contents of `/*! ... */` as SQL.
        return Err("'/*!' comments are executed by some databases".into());
    }
    let mut i = start + 2;
    while i < chars.len() {
        let pair = (chars[i], chars.get(i + 1).copied());
        if pair == ('*', Some('/')) {
            return Ok(i + 2);
        }
        if pair == ('/', Some('*')) {
            // Some databases nest comments and some do not, so the comment
            // would end in different places.
            return Err("nested block comments end differently across databases".into());
        }
        i += 1;
    }
    Err("unterminated block comment".into())
}

/// Skip a string or quoted name delimited by `quote` (one of ' " `).
/// A doubled quote inside is the standard escape. Returns the index after it.
fn skip_quoted(chars: &[char], start: usize) -> Result<usize, String> {
    let quote = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1) == Some(&quote) {
            // Some databases treat backslash-quote as an escaped quote and
            // others end the string here.
            return Err("backslash before a quote is read differently across databases".into());
        }
        if chars[i] == quote {
            if chars.get(i + 1) == Some(&quote) {
                i += 2; // doubled quote: still inside
                continue;
            }
            return Ok(i + 1);
        }
        i += 1;
    }
    Err(format!("unterminated {quote} quote"))
}

/// Skip `[ ... ]`. This is a quoted name in SQLite and SQL Server but an
/// array index elsewhere, so it is only accepted when the contents would
/// mean the same thing either way. Returns the index after it.
fn skip_bracketed(chars: &[char], start: usize) -> Result<usize, String> {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            ']' => return Ok(i + 1),
            '\'' | '"' | '`' | ';' | '[' | '-' | '/' | '$' | '#' => {
                return Err("'[...]' with punctuation inside is read differently across databases".into());
            }
            _ => i += 1,
        }
    }
    Err("unterminated '['".into())
}

/// Skip a `$name` or `$1` parameter. Refuses `$$` and `$tag$`, which start
/// a string in PostgreSQL but not elsewhere. Returns the index after it.
fn skip_dollar(chars: &[char], start: usize) -> Result<usize, String> {
    let mut i = start + 1;
    while i < chars.len() && is_word_char(chars[i]) {
        i += 1;
    }
    if chars.get(i) == Some(&'$') {
        return Err("dollar-quoted strings are read differently across databases".into());
    }
    Ok(i)
}

// ---------------------------------------------------------------------------
// Classifier
// ---------------------------------------------------------------------------

/// Words that mean "this changes something" when they appear anywhere in a
/// statement that otherwise looks like a read. This catches writes inside
/// `WITH ...`, `SELECT ... INTO`, and databases that allow a second statement
/// without a semicolon. A column that happens to share one of these names
/// must be quoted to get past; that is the price of failing closed.
const CHANGE_WORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "INTO", "CREATE", "DROP", "ALTER", "TRUNCATE", "GRANT", "REVOKE", "ATTACH",
    "DETACH", "PRAGMA", "VACUUM", "REINDEX", "CALL", "EXEC", "EXECUTE",
];

fn classify_statement(tokens: &[Token]) -> Verdict {
    // `((SELECT 1))` is legal in some databases; look past leading parens.
    let start = tokens
        .iter()
        .position(|t| *t != Token::OpenParen)
        .unwrap_or(tokens.len());
    let tokens = &tokens[start..];

    let Some(Token::Word(first)) = tokens.first() else {
        return Verdict::new(Kind::Unknown, "does not start with a keyword");
    };

    match first.as_str() {
        "SELECT" | "VALUES" | "WITH" => scan_read(tokens),

        // These only describe things. They are not scanned for change words
        // because `SHOW CREATE TABLE` is a legitimate read.
        "SHOW" | "DESCRIBE" | "DESC" => Verdict::new(Kind::Read, "read"),

        "EXPLAIN" => {
            // Some databases execute the statement under EXPLAIN ANALYZE, so
            // an EXPLAIN is only as safe as the statement it explains.
            let options = ["QUERY", "PLAN", "ANALYZE", "VERBOSE", "EXTENDED", "FORMATTED"];
            let rest = tokens[1..]
                .iter()
                .position(|t| !matches!(t, Token::Word(w) if options.contains(&w.as_str())))
                .map_or(&[][..], |n| &tokens[1 + n..]);
            classify_statement(rest)
        }

        "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "MERGE" | "UPSERT" | "COPY" | "LOAD" => {
            Verdict::new(Kind::Write, format!("{first} changes data"))
        }

        "CREATE" | "DROP" | "ALTER" | "TRUNCATE" | "RENAME" | "COMMENT" | "GRANT" | "REVOKE" => {
            Verdict::new(Kind::Ddl, format!("{first} changes structure or permissions"))
        }

        "BEGIN" | "START" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "SET" | "RESET" | "USE"
        | "PRAGMA" | "ATTACH" | "DETACH" | "VACUUM" | "ANALYZE" | "REINDEX" | "LOCK" => {
            Verdict::new(Kind::Session, format!("{first} changes session or database state"))
        }

        _ => Verdict::new(Kind::Unknown, format!("{first} is not a recognised read statement")),
    }
}

/// A statement that starts like a read is a read only if no change word
/// appears anywhere in it.
fn scan_read(tokens: &[Token]) -> Verdict {
    for (i, token) in tokens.iter().enumerate() {
        let Token::Word(word) = token else { continue };

        // `replace(x, 'a', 'b')` is a common string function; `REPLACE INTO`
        // is a write. The opening paren tells them apart.
        let is_replace_statement = word == "REPLACE" && tokens.get(i + 1) != Some(&Token::OpenParen);

        if is_replace_statement || CHANGE_WORDS.contains(&word.as_str()) {
            return Verdict::new(Kind::Write, format!("contains {word}, which can change data"));
        }
    }
    Verdict::new(Kind::Read, "read")
}
