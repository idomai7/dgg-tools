//! Who may change a machine install's files.
//!
//! The service runs as its own account, and other users may be on the
//! machine. Anything the service runs or writes to must be changeable only
//! by administrators (and SYSTEM): a replaced binary would run as the
//! service, and a directory swapped for a junction would send its writes
//! anywhere. So the root is made writable only by them, readable by every
//! user, and the service's account may change only the data and logs. A
//! root someone else created, or one with links in it, is refused.
//!
//! Permissions are set with `icacls`, owners read through PowerShell.

use {
  crate::legacy::powershell,
  ::std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
  },
};

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const USERS: &str = "S-1-5-32-545";
/// The integrity level of an elevated process.
const HIGH_INTEGRITY: &str = "S-1-16-12288";

/// Whether this process runs elevated (as an administrator).
pub fn elevated() -> bool {
  Command::new("whoami.exe")
    .arg("/groups")
    .stdin(Stdio::null())
    .output()
    .is_ok_and(|output| {
      String::from_utf8_lossy(&output.stdout).contains(HIGH_INTEGRITY)
    })
}

fn icacls(args: &[&str]) -> io::Result<()> {
  let output = Command::new("icacls.exe")
    .args(args)
    .stdin(Stdio::null())
    .output()?;
  if output.status.success() {
    Ok(())
  } else {
    Err(io::Error::other(format!(
      "icacls {}: {}",
      args.join(" "),
      String::from_utf8_lossy(&output.stdout).trim()
    )))
  }
}

/// The SID owning `path`.
pub fn owner(path: &Path) -> io::Result<String> {
  let text = path.to_string_lossy().replace('\'', "''");
  powershell(&format!(
    "(Get-Item -LiteralPath '{text}' -Force).GetAccessControl('Owner')\
     .GetOwner([Security.Principal.SecurityIdentifier]).Value"
  ))?
  .map(|sid| sid.trim().to_owned())
  .ok_or_else(|| {
    io::Error::other(format!("can't read {}'s owner", path.display()))
  })
}

/// Junctions, symlinks and other reparse points under `path`, without
/// following them.
pub fn links(path: &Path) -> io::Result<Vec<PathBuf>> {
  const REPARSE_POINT: u32 = 0x400;
  let mut found = Vec::new();
  let mut pending = vec![path.to_owned()];
  while let Some(dir) = pending.pop() {
    for entry in fs::read_dir(&dir)? {
      let entry = entry?;
      let metadata = fs::symlink_metadata(entry.path())?;
      let attributes = {
        use ::std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
      };
      if attributes & REPARSE_POINT != 0 {
        found.push(entry.path());
      } else if metadata.is_dir() {
        pending.push(entry.path());
      }
    }
  }
  Ok(found)
}

/// Makes `root` writable only by administrators and SYSTEM, readable by
/// every user, everything under it inheriting that (and owned by
/// administrators). Refuses a `root` someone else owns, or one with links
/// in it.
pub fn protect(root: &Path) -> io::Result<()> {
  if root.exists() {
    let owner = owner(root)?;
    if owner != ADMINISTRATORS && owner != SYSTEM {
      return Err(io::Error::other(format!(
        "{} exists and isn't owned by Administrators or SYSTEM, so someone \
         else may have prepared it; check it, then delete it or take \
         ownership of it",
        root.display()
      )));
    }
    let links = links(root)?;
    if !links.is_empty() {
      return Err(io::Error::other(format!(
        "{} has links in it, which the service would follow: {}",
        root.display(),
        links
          .iter()
          .map(|link| link.display().to_string())
          .collect::<Vec<_>>()
          .join(", ")
      )));
    }
  } else {
    fs::create_dir_all(root)?;
  }
  let text = root.to_string_lossy();
  icacls(&[&text, "/setowner", &format!("*{ADMINISTRATORS}"), "/Q"])?;
  icacls(&[
    &text,
    "/inheritance:r",
    "/grant:r",
    &format!("*{SYSTEM}:(OI)(CI)F"),
    &format!("*{ADMINISTRATORS}:(OI)(CI)F"),
    &format!("*{USERS}:(OI)(CI)RX"),
    "/Q",
  ])?;
  if fs::read_dir(root)?.next().is_some() {
    let children = format!("{text}\\*");
    icacls(&[&children, "/reset", "/T", "/C", "/Q"])?;
    icacls(&[
      &children,
      "/setowner",
      &format!("*{ADMINISTRATORS}"),
      "/T",
      "/C",
      "/Q",
    ])?;
  }
  Ok(())
}

/// Lets `account` change `path` and everything under it.
pub fn grant(path: &Path, account: &str) -> io::Result<()> {
  icacls(&[
    &path.to_string_lossy(),
    "/grant",
    &format!("{account}:(OI)(CI)M"),
    "/Q",
  ])
}

/// Undoes [`protect`] for `path`: it inherits from its parent again, and
/// `user` owns it.
pub fn release(path: &Path, user: &str) -> io::Result<()> {
  if !path.exists() {
    return Ok(());
  }
  let text = path.to_string_lossy();
  icacls(&[&text, "/reset", "/T", "/C", "/Q"])?;
  icacls(&[&text, "/setowner", user, "/T", "/C", "/Q"])
}
