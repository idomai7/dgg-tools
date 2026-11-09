//! For build scripts: gives each Windows binary a version resource, so
//! Task Manager and Explorer show what it is ("dgg-tools: capture chat into
//! the journal") and which version, rather than just its file name.
//!
//! ```ignore
//! fn main() {
//!   ::version_info::embed(&[("dgg-capture", "capture chat into the journal")]);
//! }
//! ```
//!
//! Does nothing when the target isn't Windows.

use ::std::{env, fs, path::PathBuf};

/// The product every binary belongs to, before each description.
pub const PRODUCT: &str = "dgg-tools";

/// Embeds a version resource in each of `bins`, `(name, description)`, of
/// the package being built, with the package's version.
pub fn embed(bins: &[(&str, &str)]) {
  if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
    return;
  }
  let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
  let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
  for (name, description) in bins {
    let path = out.join(format!("{name}.rc"));
    fs::write(&path, resource(name, description, &version))
      .expect("writing a resource script");
    ::embed_resource::compile_for(&path, [name], ::embed_resource::NONE)
      .manifest_optional()
      .expect("compiling a version resource");
  }
}

/// A resource script with one `VERSIONINFO` for `name`.
pub fn resource(name: &str, description: &str, version: &str) -> String {
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
     \x20     VALUE \"FileVersion\", \"{version}\"\n\
     \x20     VALUE \"InternalName\", \"{name}\"\n\
     \x20     VALUE \"OriginalFilename\", \"{name}.exe\"\n\
     \x20     VALUE \"ProductName\", \"{PRODUCT}\"\n\
     \x20     VALUE \"ProductVersion\", \"{version}\"\n\
     \x20   END\n\
     \x20 END\n\
     \x20 BLOCK \"VarFileInfo\"\n\
     \x20 BEGIN\n\
     \x20   VALUE \"Translation\", 0x409, 1200\n\
     \x20 END\n\
     END\n"
  )
}
