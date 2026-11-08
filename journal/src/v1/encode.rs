use {
  super::{Body, Record},
  ::base64::{Engine, engine::general_purpose::STANDARD as BASE64},
};

impl Record {
  /// Appends this record's line, newline included, to `out`.
  pub fn encode(&self, out: &mut Vec<u8>) {
    let (kind, data) = match &self.body {
      Body::Frame(t) => ("frame", t.as_bytes()),
      Body::Open(t) => ("open", t.as_bytes()),
      Body::Close(t) => ("close", t.as_bytes()),
      Body::Binary(b) => ("binary", b.as_slice()),
    };
    let binary = matches!(self.body, Body::Binary(_));
    let text64 = !binary && data.contains(&b'\n');
    let suffix = if text64 { "64" } else { "" };
    let header =
      format!("{} {} {} {kind}{suffix}", self.recv, self.inst, self.conn);
    out.extend_from_slice(header.as_bytes());
    if !data.is_empty() {
      out.push(b' ');
      if binary || text64 {
        out.extend_from_slice(BASE64.encode(data).as_bytes());
      } else {
        out.extend_from_slice(data);
      }
    }
    out.push(b'\n');
  }
}
