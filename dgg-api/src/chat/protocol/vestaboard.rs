//! Vestaboard tiles: the board in destiny.gg's office, 6 rows of 22, whose
//! designs chat announces.
//!
//! Each tile is an integer code, mapped as chat-gui's
//! `vestaboardCharacters.js` maps them (itself ported from the website's
//! `Destiny\Vestaboard\VestaboardCharacters`).

/// Rows on the board.
pub const ROWS: usize = 6;
/// Tiles in a row.
pub const COLUMNS: usize = 22;

/// A spot-colour tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
  Red,
  Orange,
  Yellow,
  Green,
  Blue,
  Violet,
  White,
  Black,
  /// A filled tile, which chat-gui draws grey.
  Filled,
}

/// One tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
  Blank,
  Char(char),
  Color(Color),
  /// A code with no mapping (chat-gui draws it blank).
  Unknown(u8),
}

/// Codes 37 to 60, in order; spaces are codes with no mapping.
const PUNCTUATION: &[u8; 24] = b"!@#$() - +&=;: '\"%,.  /?";

impl Tile {
  pub fn from_code(code: u8) -> Self {
    match code {
      0 => Self::Blank,
      1..=26 => Self::Char(char::from(b'A' + code - 1)),
      27..=35 => Self::Char(char::from(b'1' + code - 27)),
      36 => Self::Char('0'),
      37..=60 => match PUNCTUATION[usize::from(code - 37)] {
        b' ' => Self::Unknown(code),
        byte => Self::Char(char::from(byte)),
      },
      63 => Self::Color(Color::Red),
      64 => Self::Color(Color::Orange),
      65 => Self::Color(Color::Yellow),
      66 => Self::Color(Color::Green),
      67 => Self::Color(Color::Blue),
      68 => Self::Color(Color::Violet),
      69 => Self::Color(Color::White),
      70 => Self::Color(Color::Black),
      71 => Self::Color(Color::Filled),
      _ => Self::Unknown(code),
    }
  }
}
