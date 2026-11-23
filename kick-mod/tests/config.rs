//! The settings file. Ids and secrets are made up.

use ::kick_mod::config::Config;

#[test]
fn reads_the_app_and_the_server() {
  let config = Config::parse(
    r#"
    [app]
    client_id = "app-id"
    client_secret = "app-secret"

    [server]
    listen = "127.0.0.1:9000"
    "#,
  )
  .unwrap();
  assert_eq!(config.app.client_id, "app-id");
  assert_eq!(config.app.client_secret, "app-secret");
  assert_eq!(config.server.listen.to_string(), "127.0.0.1:9000");
  assert_eq!(config.app().client_id, "app-id");
  assert!(!format!("{config:?}").contains("app-secret"));
}

#[test]
fn serves_on_8787_by_default() {
  let app = "[app]\nclient_id = \"a\"\nclient_secret = \"s\"\n";
  let config = Config::parse(app).unwrap();
  assert_eq!(config.server.listen.to_string(), "127.0.0.1:8787");
  let config = Config::parse(&format!("{app}[server]\n")).unwrap();
  assert_eq!(config.server.listen.to_string(), "127.0.0.1:8787");
}

#[test]
fn refuses_what_it_doesnt_know() {
  let app = "[app]\nclient_id = \"a\"\nclient_secret = \"s\"\n";
  // No app, a missing secret, a misspelt field.
  assert!(Config::parse("").is_err());
  assert!(Config::parse("[app]\nclient_id = \"a\"\n").is_err());
  assert!(
    Config::parse(&format!("{app}[server]\nlisten_on = \"x\"\n")).is_err()
  );
  // Only a loopback address, while there's no login.
  let open = format!("{app}[server]\nlisten = \"0.0.0.0:8787\"\n");
  assert!(Config::parse(&open).unwrap_err().contains("loopback"));
  let ipv6 = format!("{app}[server]\nlisten = \"[::1]:8787\"\n");
  assert!(Config::parse(&ipv6).is_ok());
}
