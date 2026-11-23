//! Running PowerShell, where there is one.

#[cfg(windows)]
#[test]
fn runs_a_command() {
  use ::common::powershell::run;
  let out = run("Write-Output 'héllo'").unwrap().unwrap();
  assert_eq!(out.trim_end(), "héllo");
  assert_eq!(run("exit 3").unwrap(), None);
}
