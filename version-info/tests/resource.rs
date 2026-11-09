use ::version_info::resource;

#[test]
fn describes_the_binary() {
  let script =
    resource("dgg-capture", "capture chat into the journal", "0.1.0");
  assert!(script.starts_with("1 VERSIONINFO\nFILEVERSION 0,1,0,0\n"));
  assert!(script.contains(
    "VALUE \"FileDescription\", \"dgg-tools: capture chat into the journal\""
  ));
  assert!(script.contains("VALUE \"OriginalFilename\", \"dgg-capture.exe\""));
  assert!(script.contains("VALUE \"ProductVersion\", \"0.1.0\""));
}

#[test]
fn numbers_prerelease_versions_and_quotes() {
  let script = resource("x", "say \"hi\"", "1.2.3-alpha.4");
  assert!(script.contains("FILEVERSION 1,2,3,0\n"));
  assert!(script.contains("\"dgg-tools: say \"\"hi\"\"\""));
}
