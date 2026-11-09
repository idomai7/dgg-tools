fn main() {
  ::version_info::embed(&[
    ("dgg-backfill", "backfill chat from other logs"),
    ("dgg-capture", "capture chat into the journal"),
    ("dgg-compact", "compact the journal"),
    ("dgg-gaps", "list gaps in the capture"),
    ("dgg-index", "index the journal"),
    ("dgg-serve", "serve the journal to other hosts"),
    ("dgg-sync", "sync the journal from another host"),
  ]);
}
