//! The schemas/ directory keeps every index schema version; the newest is
//! the one the index uses.

use {
  ::journal::Version,
  ::logger::index::{SCHEMA_VERSION, open},
  ::rusqlite::Connection,
  ::std::{fs, path::PathBuf},
};

fn schema_file(version: Version) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("schemas/index")
    .join(format!("v{}.{}.sql", version.major, version.minor))
}

/// Every table and index a database defines, as SQLite stored it.
fn definitions(db: &Connection) -> Vec<String> {
  let mut query = db
    .prepare(
      "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
    )
    .unwrap();
  query
    .query_map([], |row| row.get(0))
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn every_version_is_recorded() {
  // Major 0 started at 0.1.
  for minor in 1..=SCHEMA_VERSION.minor {
    let version = Version { major: 0, minor };
    let sql = fs::read_to_string(schema_file(version))
      .unwrap_or_else(|e| panic!("schema 0.{minor}: {e}"));
    Connection::open_in_memory()
      .unwrap()
      .execute_batch(&sql)
      .unwrap_or_else(|e| panic!("schema 0.{minor} doesn't run: {e}"));
  }
  let next = schema_file(Version {
    minor: SCHEMA_VERSION.minor + 1,
    ..SCHEMA_VERSION
  });
  assert!(!next.exists(), "{next:?} exists; bump SCHEMA_VERSION");
}

#[test]
fn newest_is_what_open_creates() {
  let directory = ::std::env::temp_dir()
    .join(format!("logger-schema-test-{}", ::std::process::id()));
  let _ = fs::remove_dir_all(&directory);
  fs::create_dir_all(&directory).unwrap();
  let opened = open(&directory.join("index.sqlite")).unwrap();

  let sql = fs::read_to_string(schema_file(SCHEMA_VERSION)).unwrap();
  let expected = Connection::open_in_memory().unwrap();
  expected.execute_batch(&sql).unwrap();
  assert_eq!(definitions(&opened), definitions(&expected));
  drop(opened);
  let _ = fs::remove_dir_all(&directory);
}
