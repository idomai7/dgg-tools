//! For build scripts: names the build by its commit, and gives each Windows
//! binary a version resource, so Task Manager and Explorer show what it is
//! ("dgg-tools: the chat logger") and which build, rather than just its
//! file name.
//!
//! ```ignore
//! fn main() {
//!   ::version_info::export();
//!   ::version_info::embed(&[("dgg-logger", "the chat logger")]);
//! }
//! ```
//!
//! [`embed`] does nothing when the target isn't Windows.

use ::std::{
  env, fs,
  path::{Path, PathBuf},
  process::Command,
};

/// The product every binary belongs to, before each description.
pub const PRODUCT: &str = "dgg-tools";

/// The environment variable [`export`] sets for the crate being built.
pub const BUILD: &str = "DGG_TOOLS_BUILD";

/// Runs git in `dir`; its output, trimmed, if it succeeds.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
  let output = Command::new("git")
    .current_dir(dir)
    .args(args)
    .output()
    .ok()?;
  output
    .status
    .success()
    .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The build's name: the commit being built (12 hex digits), with `-dirty`
/// if tracked files differ from it; `unknown` outside a git checkout. Asks
/// cargo to run the build script again when the commit or any workspace
/// crate's files change, so the name stays true.
pub fn commit() -> String {
  let dir =
    PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
  let Some(top) = git(&dir, &["rev-parse", "--show-toplevel"]) else {
    return "unknown".into();
  };
  let top = PathBuf::from(top);
  if let Some(git_dir) = git(&dir, &["rev-parse", "--absolute-git-dir"]) {
    let git_dir = PathBuf::from(git_dir);
    for watched in ["HEAD", "index", "refs/heads", "packed-refs"] {
      println!("cargo:rerun-if-changed={}", git_dir.join(watched).display());
    }
  }
  // Every crate in the checkout (a directory with a Cargo.toml), not
  // target/, whose files change with each build.
  for entry in fs::read_dir(&top).into_iter().flatten().flatten() {
    let path = entry.path();
    if path.join("Cargo.toml").exists() {
      println!("cargo:rerun-if-changed={}", path.display());
    }
  }
  let Some(commit) = git(&dir, &["rev-parse", "--short=12", "HEAD"]) else {
    return "unknown".into();
  };
  let status = git(&top, &["status", "--porcelain", "--untracked-files=no"]);
  match status {
    Some(status) if status.is_empty() => commit,
    _ => format!("{commit}-dirty"),
  }
}

/// Sets [`BUILD`] to the [`commit`] for the crate being built, for
/// `env!("DGG_TOOLS_BUILD")`.
pub fn export() {
  println!("cargo:rustc-env={BUILD}={}", commit());
}

/// Embeds a version resource in each of `bins`, `(name, description)`, of
/// the package being built: numbered with the package's version, and
/// naming the [`commit`] as its version.
pub fn embed(bins: &[(&str, &str)]) {
  if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
    return;
  }
  let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
  let commit = commit();
  let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
  for (name, description) in bins {
    let path = out.join(format!("{name}.rc"));
    fs::write(&path, resource(name, description, &version, &commit))
      .expect("writing a resource script");
    ::embed_resource::compile_for(&path, [name], ::embed_resource::NONE)
      .manifest_optional()
      .expect("compiling a version resource");
  }
}

/// A resource script with one `VERSIONINFO` for `name`: numbered from
/// `version`, with `build` as its version text.
pub fn resource(
  name: &str,
  description: &str,
  version: &str,
  build: &str,
) -> String {
  let mut numbers: Vec<u16> = version
    .split(['.', '-', '+'])
    .map_while(|part| part.parse().ok())
    .take(4)
    .collect();
  numbers.resize(4, 0);
  let numbers = numbers
    .iter()
    .map(u16::to_string)
    .collect::<Vec<_>>()
    .join(",");
  let quote = |text: &str| text.replace('"', "\"\"");
  let description = quote(&format!("{PRODUCT}: {description}"));
  let name = quote(name);
  let build = quote(build);
  format!(
    "1 VERSIONINFO\n\
     FILEVERSION {numbers}\n\
     PRODUCTVERSION {numbers}\n\
     FILEOS 0x40004\n\
     FILETYPE 0x1\n\
     BEGIN\n\
     \x20 BLOCK \"StringFileInfo\"\n\
     \x20 BEGIN\n\
     \x20   BLOCK \"040904B0\"\n\
     \x20   BEGIN\n\
     \x20     VALUE \"CompanyName\", \"{PRODUCT}\"\n\
     \x20     VALUE \"FileDescription\", \"{description}\"\n\
     \x20     VALUE \"FileVersion\", \"{build}\"\n\
     \x20     VALUE \"InternalName\", \"{name}\"\n\
     \x20     VALUE \"OriginalFilename\", \"{name}.exe\"\n\
     \x20     VALUE \"ProductName\", \"{PRODUCT}\"\n\
     \x20     VALUE \"ProductVersion\", \"{build}\"\n\
     \x20   END\n\
     \x20 END\n\
     \x20 BLOCK \"VarFileInfo\"\n\
     \x20 BEGIN\n\
     \x20   VALUE \"Translation\", 0x409, 1200\n\
     \x20 END\n\
     END\n"
  )
}
