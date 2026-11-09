//! The emotes' and flairs' stylesheets, split into what the site generates
//! from the JSON and what it doesn't, checking the generated part against
//! the JSON.
//!
//! The website (`Destiny\Chat\EmoteService` and `FlairService`) writes
//! them whenever the assets change:
//!
//! - `emotes.css`: two rules per emote, in `emotes.json`'s order, then each
//!   emote's hand-written styles (with `{PREFIX}` replaced by its prefix),
//!   in the same order, run together: nothing marks where one emote's end.
//! - `flairs.css`: a fixed preamble (the rainbow), then one block per
//!   flair, in reverse of `flairs.json`'s order: its badge, then the
//!   username colour it gives, if any.
//!
//! What's checked is each rule's selector and declarations, not its bytes:
//! the site has changed its indentation before.

use {
  super::{Emote, Flair},
  ::std::{error, fmt},
};

/// `emotes.css`, split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmotesCss<'a> {
  /// Before the generated rules; seen empty.
  pub preamble: &'a str,
  /// The emotes' hand-written styles: animations, hover effects, and rules
  /// for emotes next to each other, combos, sub tiers, even single users.
  /// From its first non-space.
  pub styles: &'a str,
}

/// `flairs.css`, split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlairsCss<'a> {
  /// Before the generated rules: the rainbow's variables and keyframes.
  pub preamble: &'a str,
  /// After them, from its first non-space; seen empty.
  pub rest: &'a str,
}

/// The stylesheet doesn't have what the JSON says it should.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
  /// Where in the stylesheet.
  pub offset: usize,
  /// What was expected there.
  pub expected: String,
}

impl fmt::Display for Mismatch {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "at byte {}, expected {}", self.offset, self.expected)
  }
}

impl error::Error for Mismatch {}

/// Splits `css` (`emotes.css`), checking its generated rules against
/// `emotes` (the matching `emotes.json`).
pub fn emotes<'a>(
  css: &'a str,
  emotes: &[Emote],
) -> Result<EmotesCss<'a>, Mismatch> {
  let Some(first) = emotes.first() else {
    return Ok(EmotesCss {
      preamble: "",
      styles: css,
    });
  };
  let start = find_rule(css, &format!(".emote.{}", first.prefix));
  let mut reader = Reader { css, at: start };
  for emote in emotes {
    let class = format!(".emote.{}", emote.prefix);
    let image = emote.image();
    let (file, width, height) = image.map_or(("", 0, 0), |image| {
      (image.name.as_str(), image.width, image.height)
    });
    let height = f64::from(height);
    reader.expect(
      &class,
      &[
        ("background-image", Value::Url(file)),
        ("height", Value::Px(height)),
        ("width", Value::Px(f64::from(width))),
      ],
    )?;
    reader.expect(
      &format!(".msg-chat {class}"),
      &[
        ("margin-top", Value::Px(-height)),
        ("top", Value::Px(height / 4.0)),
      ],
    )?;
  }
  Ok(EmotesCss {
    preamble: &css[..start],
    styles: css[reader.at..].trim_start(),
  })
}

/// Splits `css` (`flairs.css`), checking its generated rules against
/// `flairs` (the matching `flairs.json`).
pub fn flairs<'a>(
  css: &'a str,
  flairs: &[Flair],
) -> Result<FlairsCss<'a>, Mismatch> {
  let Some(first) = flairs.last() else {
    return Ok(FlairsCss {
      preamble: css,
      rest: "",
    });
  };
  let start = find_rule(css, &format!(".flair.{}", first.name));
  let mut reader = Reader { css, at: start };
  for flair in flairs.iter().rev() {
    let class = format!(".flair.{}", flair.name);
    match (flair.hidden, flair.image()) {
      (true, _) => {
        reader.expect(&class, &[("display", Value::Text("none !important"))])?
      }
      (false, image) => {
        let (file, width, height) = image.map_or(("", 0, 0), |image| {
          (image.name.as_str(), image.width, image.height)
        });
        reader.expect(
          &class,
          &[
            ("background-image", Value::Url(file)),
            ("height", Value::Px(f64::from(height))),
            ("width", Value::Px(f64::from(width))),
            ("order", Value::Px(flair.priority as f64)),
          ],
        )?
      }
    }
    let user = format!(".user.{}", flair.name);
    if flair.rainbow_color {
      // The rainbow's own rules: its gradient, and an underline on hover.
      let at = reader.at;
      let mut count = 0;
      while reader.next_targets(&user) {
        reader.rule();
        count += 1;
      }
      if count == 0 {
        return Err(Mismatch {
          offset: at,
          expected: format!("`{user}`'s rainbow"),
        });
      }
    } else if let Some(color) = &flair.color {
      reader.expect(&user, &[("color", Value::Text(color))])?;
    }
  }
  Ok(FlairsCss {
    preamble: &css[..start],
    rest: css[reader.at..].trim_start(),
  })
}

/// Where a rule for exactly `selector` starts, or the end if none does.
fn find_rule(css: &str, selector: &str) -> usize {
  let mut reader = Reader { css, at: 0 };
  loop {
    let at = reader.skip_space();
    match reader.rule() {
      Some(rule) if same_selector(rule.selector, selector) => return at,
      Some(_) => {}
      None => return css.len(),
    }
  }
}

fn same_selector(found: &str, expected: &str) -> bool {
  found.split_whitespace().eq(expected.split_whitespace())
}

/// A declaration's expected value.
enum Value<'a> {
  Text(&'a str),
  /// A length in px (or a plain number), compared as a number.
  Px(f64),
  /// `url("...")`.
  Url(&'a str),
}

impl Value<'_> {
  fn matches(&self, found: &str) -> bool {
    match self {
      Self::Text(text) => found == *text,
      Self::Px(number) => found
        .strip_suffix("px")
        .unwrap_or(found)
        .parse::<f64>()
        .is_ok_and(|found| found == *number),
      Self::Url(file) => found == format!("url(\"{file}\")"),
    }
  }
}

impl fmt::Display for Value<'_> {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::Text(text) => f.write_str(text),
      Self::Px(number) => write!(f, "{number}px"),
      Self::Url(file) => write!(f, "url(\"{file}\")"),
    }
  }
}

struct Rule<'a> {
  selector: &'a str,
  body: &'a str,
}

impl<'a> Rule<'a> {
  /// Its declarations, `name: value`, trimmed.
  fn declarations(&self) -> Vec<(&'a str, &'a str)> {
    split_outside_quotes(self.body, ';')
      .into_iter()
      .filter_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        Some((name.trim(), value.trim()))
      })
      .collect()
  }
}

/// Reads rules one after another.
struct Reader<'a> {
  css: &'a str,
  at: usize,
}

impl<'a> Reader<'a> {
  /// Skips whitespace and comments; returns where they end.
  fn skip_space(&mut self) -> usize {
    loop {
      let rest = &self.css[self.at..];
      let trimmed = rest.trim_start();
      self.at += rest.len() - trimmed.len();
      match trimmed.strip_prefix("/*") {
        Some(comment) => {
          self.at += 2 + comment.find("*/").map_or(comment.len(), |end| end + 2)
        }
        None => return self.at,
      }
    }
  }

  /// The next rule (an at-rule's block whole), if any.
  fn rule(&mut self) -> Option<Rule<'a>> {
    self.skip_space();
    let rest = &self.css[self.at..];
    let open = rest.find('{')?;
    let mut depth = 0;
    let mut close = None;
    for (index, byte) in rest.bytes().enumerate().skip(open) {
      match byte {
        b'{' => depth += 1,
        b'}' => {
          depth -= 1;
          if depth == 0 {
            close = Some(index);
            break;
          }
        }
        _ => {}
      }
    }
    let close = close?;
    self.at += close + 1;
    Some(Rule {
      selector: rest[..open].trim(),
      body: &rest[open + 1..close],
    })
  }

  /// Whether the next rule's selectors all target `class` (it, or it with
  /// pseudo-classes and context).
  fn next_targets(&mut self, class: &str) -> bool {
    let at = self.at;
    let targets = self.rule().is_some_and(|rule| {
      split_outside_quotes(rule.selector, ',')
        .iter()
        .all(|selector| {
          selector.match_indices(class).any(|(index, _)| {
            let after = &selector[index + class.len()..];
            !after.starts_with(|c: char| {
              c.is_alphanumeric() || c == '_' || c == '-'
            })
          })
        })
    });
    self.at = at;
    targets
  }

  /// Reads a rule for exactly `selector` with exactly `declarations`.
  fn expect(
    &mut self,
    selector: &str,
    declarations: &[(&str, Value)],
  ) -> Result<(), Mismatch> {
    let offset = self.skip_space();
    let expected = || Mismatch {
      offset,
      expected: format!(
        "`{selector} {{ {} }}`",
        declarations
          .iter()
          .map(|(name, value)| format!("{name}: {value};"))
          .collect::<Vec<_>>()
          .join(" ")
      ),
    };
    let rule = self.rule().ok_or_else(expected)?;
    let found = rule.declarations();
    let matches = same_selector(rule.selector, selector)
      && found.len() == declarations.len()
      && declarations.iter().all(|(name, value)| {
        found
          .iter()
          .any(|(found_name, found)| found_name == name && value.matches(found))
      });
    match matches {
      true => Ok(()),
      false => Err(expected()),
    }
  }
}

/// `text` split at `separator`s outside quotes and parentheses.
fn split_outside_quotes(text: &str, separator: char) -> Vec<&str> {
  let mut parts = Vec::new();
  let (mut start, mut depth, mut quote) = (0, 0, None);
  for (index, c) in text.char_indices() {
    match (quote, c) {
      (Some(open), c) if c == open => quote = None,
      (Some(_), _) => {}
      (None, '"' | '\'') => quote = Some(c),
      (None, '(') => depth += 1,
      (None, ')') => depth -= 1,
      (None, c) if c == separator && depth == 0 => {
        parts.push(&text[start..index]);
        start = index + c.len_utf8();
      }
      _ => {}
    }
  }
  parts.push(&text[start..]);
  parts.retain(|part| !part.trim().is_empty());
  parts
}
