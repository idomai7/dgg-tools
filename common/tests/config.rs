//! Configs and secrets as JSON.

use {
  ::common::{
    config::{MAX_SIZE, Secret, Source, parse, read},
    temp_dir::TempDir,
  },
  ::serde::{Deserialize, Serialize},
  ::std::fs,
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
  url: String,
  token: Secret,
  other: Option<Secret>,
}

const TOKEN: &str = "a-made-up-token-for-tests-only-x";

#[test]
fn a_secret_never_prints() {
  let config: Config =
    parse(&format!(r#"{{"url": "https://host", "token": "{TOKEN}"}}"#))
      .unwrap();
  assert_eq!(config.token.expose(), TOKEN);
  assert!(config.other.is_none());
  let debug = format!("{config:?}");
  assert!(!debug.contains(TOKEN), "{debug}");
  assert!(debug.contains("<secret>"), "{debug}");
  let shown = ::serde_json::to_string(&config).unwrap();
  assert_eq!(
    shown,
    r#"{"url":"https://host","token":"<secret>","other":null}"#
  );
}

#[test]
fn errors_name_the_field_but_never_quote_a_value() {
  let error = |text: &str| parse::<Config>(text).unwrap_err();
  let missing = error(r#"{"url": "https://host"}"#);
  assert!(missing.contains("missing field `token`"), "{missing}");
  let unknown = error(&format!(
    r#"{{"url": "u", "token": "{TOKEN}", "tokne": "{TOKEN}"}}"#
  ));
  assert!(unknown.contains("unknown field `tokne`"), "{unknown}");
  assert!(!unknown.contains(TOKEN), "{unknown}");
  // A secret where a different type belongs is never quoted.
  let wrong = error(&format!(r#"{{"url": "u", "token": ["{TOKEN}"]}}"#));
  assert!(
    wrong.starts_with("a value of the wrong type at line 1"),
    "{wrong}"
  );
  let wrong = error(&format!(r#"{{"url": {{"x": "{TOKEN}"}}, "token": 1}}"#));
  assert!(!wrong.contains(TOKEN), "{wrong}");
  let broken = error(&format!(r#"{{"url": "u", "token": "{TOKEN}""#));
  assert!(!broken.contains(TOKEN), "{broken}");
}

#[test]
fn reads_a_file() {
  let dir = TempDir::new("common-config");
  let path = dir.path().join("config.json");
  fs::write(&path, format!(r#"{{"url": "u", "token": "{TOKEN}"}}"#)).unwrap();
  let config: Config = read(Source::File(&path)).unwrap();
  assert_eq!(config.token.expose(), TOKEN);
  // Errors say which file.
  fs::write(&path, "{}").unwrap();
  let error = read::<Config>(Source::File(&path)).unwrap_err();
  assert!(error.starts_with(&path.display().to_string()), "{error}");
  let missing = dir.path().join("missing.json");
  assert!(read::<Config>(Source::File(&missing)).is_err());
}

#[test]
fn refuses_a_file_too_large() {
  let dir = TempDir::new("common-config-large");
  let path = dir.path().join("config.json");
  fs::write(&path, " ".repeat(MAX_SIZE as usize + 1)).unwrap();
  let error = read::<Config>(Source::File(&path)).unwrap_err();
  assert!(error.contains("over"), "{error}");
}
