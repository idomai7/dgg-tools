// Decode every frame in a logger journal's live files, and count what came
// out: events by name, and any that were unreadable (each shown once) or
// untyped. For checking chat::event against what dgg really sends.
// Usage: cargo run -p dgg-api --features protocol --example decode_journal --
//          <journal dir>

use {
  ::dgg_api::chat::protocol::Event,
  ::std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
  },
};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
  for entry in fs::read_dir(dir).expect("readable directory").flatten() {
    let path = entry.path();
    if path.is_dir() {
      files(&path, out);
    } else if path
      .extension()
      .is_some_and(|extension| extension == "journal")
    {
      out.push(path);
    }
  }
}

fn count(event: &Event, counts: &mut BTreeMap<String, u64>) {
  let key = match event {
    Event::Other { event, .. } => format!("{event} (untyped)"),
    Event::Unreadable { event, error, .. } => {
      if !counts.contains_key(&format!("{event} (unreadable)")) {
        println!("unreadable {event}: {error}");
      }
      format!("{event} (unreadable)")
    }
    Event::History(inner) | Event::PaidEvents(inner) => {
      for event in inner {
        count(event, counts);
      }
      event.name().into()
    }
    _ => event.name().into(),
  };
  *counts.entry(key).or_default() += 1;
}

fn main() {
  let dir = env::args()
    .nth(1)
    .expect("usage: decode_journal <journal dir>");
  let mut paths = Vec::new();
  files(Path::new(&dir), &mut paths);
  let mut counts = BTreeMap::new();
  for path in paths {
    let text = fs::read_to_string(&path).expect("UTF-8 journal");
    // Lines: <recv> <instance> <connection> <kind> <data>.
    for line in text.lines().skip(1) {
      let mut parts = line.splitn(5, ' ').skip(3);
      if let (Some("frame"), Some(frame)) = (parts.next(), parts.next()) {
        count(&Event::parse(frame), &mut counts);
      }
    }
  }
  for (name, count) in counts {
    println!("{count:>8} {name}");
  }
}
