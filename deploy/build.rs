fn main() {
  ::version_info::export();
  ::version_info::embed(&[
    ("dgg-logger", "the chat logger"),
    ("dgg-supervisor", "keep the logger running"),
  ]);
}
