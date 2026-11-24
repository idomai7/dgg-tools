//! The settings file. Ids and secrets are made up.

use ::kick_mod::config::Config;

const APP: &str = r#""app": {"client_id": "a", "client_secret": "s"}"#;

#[test]
fn reads_the_app_the_server_and_the_logger() {
  let config = Config::parse(
    r#"{
      "app": {"client_id": "app-id", "client_secret": "app-secret"},
      "server": {"listen": "127.0.0.1:9000"},
      "logger": {"url": "http://pi:8788", "token": "made-up-query-token"}
    }"#,
  )
  .unwrap();
  assert_eq!(config.app.client_id, "app-id");
  assert_eq!(config.app.client_secret.expose(), "app-secret");
  assert_eq!(config.server.listen.to_string(), "127.0.0.1:9000");
  assert_eq!(config.app().client_id, "app-id");
  let logger = config.logger.as_ref().unwrap();
  assert_eq!(logger.url, "http://pi:8788");
  assert_eq!(logger.token.expose(), "made-up-query-token");
  assert_eq!(logger.ca, None);
  // Shown, the secrets aren't.
  let debug = format!("{config:?}");
  let shown = ::serde_json::to_string(&config).unwrap();
  for shown in [debug, shown] {
    assert!(!shown.contains("app-secret"), "{shown}");
    assert!(!shown.contains("made-up-query-token"), "{shown}");
    assert!(shown.contains("app-id"), "{shown}");
  }
}

#[test]
fn serves_on_8787_by_default() {
  let config = Config::parse(&format!("{{{APP}}}")).unwrap();
  assert_eq!(config.server.listen.to_string(), "127.0.0.1:8787");
  assert!(config.logger.is_none());
  let config = Config::parse(&format!(r#"{{{APP}, "server": {{}}}}"#));
  assert_eq!(config.unwrap().server.listen.to_string(), "127.0.0.1:8787");
}

#[test]
fn refuses_what_it_doesnt_know() {
  let with = |rest: &str| Config::parse(&format!("{{{APP}, {rest}}}"));
  // No app, a missing secret, a misspelt field.
  assert!(Config::parse("{}").is_err());
  assert!(Config::parse(r#"{"app": {"client_id": "a"}}"#).is_err());
  assert!(with(r#""server": {"listen_on": "x"}"#).is_err());
  // A value of the wrong type isn't quoted.
  let error = Config::parse(
    r#"{"app": {"client_id": "a", "client_secret": ["leaked-secret"]}}"#,
  )
  .unwrap_err();
  assert!(!error.contains("leaked-secret"), "{error}");
  // Only a loopback address, while there's no login.
  let open = with(r#""server": {"listen": "0.0.0.0:8787"}"#);
  assert!(open.unwrap_err().contains("loopback"));
  assert!(with(r#""server": {"listen": "[::1]:8787"}"#).is_ok());
}

#[test]
fn refuses_a_logger_it_cant_reach_safely() {
  let logger = |url: &str, ca: Option<&str>| {
    let ca = ca.map_or(String::new(), |ca| format!(r#", "ca": "{ca}""#));
    Config::parse(&format!(
      r#"{{{APP}, "logger": {{"url": "{url}", "token": "t"{ca}}}}}"#
    ))
  };
  let error = logger("pi:8788", None).unwrap_err();
  assert!(error.contains("isn't an http or https URL"), "{error}");
  // A CA only over HTTPS, and one that's a certificate.
  let error = logger("http://pi:8788", Some("x")).unwrap_err();
  assert!(error.contains("over HTTPS only"), "{error}");
  let error = logger("https://pi:8788", Some("not a certificate"));
  assert!(error.unwrap_err().starts_with("logger.ca: "));
  assert!(logger("https://pi:8788", None).is_ok());
}
