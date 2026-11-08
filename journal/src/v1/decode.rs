use {
  super::{Body, Record, valid_inst, valid_kind},
  crate::{DecodeError, digits},
  ::base64::{Engine, engine::general_purpose::STANDARD as BASE64},
};

impl Record {
  /// Decodes one line, without its newline.
  pub fn decode(line: &[u8]) -> Result<Self, DecodeError> {
    use DecodeError::Bad;
    let line = ::std::str::from_utf8(line).map_err(|_| Bad("not utf-8"))?;
    let mut parts = line.splitn(5, ' ');
    let mut field = || parts.next().ok_or(Bad("too few fields"));
    let (recv, inst, conn, kind) = (field()?, field()?, field()?, field()?);
    let data = parts.next().unwrap_or("");
    if !digits(recv) || !digits(conn) {
      return Err(Bad("recv/conn not decimal"));
    }
    if !valid_inst(inst) {
      return Err(Bad("bad inst"));
    }
    if !valid_kind(kind) {
      return Err(Bad("bad kind"));
    }
    let recv = recv.parse().map_err(|_| Bad("recv out of range"))?;
    let conn = conn.parse().map_err(|_| Bad("conn out of range"))?;
    let unbase64 = || BASE64.decode(data).map_err(|_| Bad("bad base64"));
    let text64 =
      || String::from_utf8(unbase64()?).map_err(|_| Bad("base64 not utf-8"));
    let body = match kind {
      "frame" => Body::Frame(data.into()),
      "open" => Body::Open(data.into()),
      "close" => Body::Close(data.into()),
      "frame64" => Body::Frame(text64()?),
      "open64" => Body::Open(text64()?),
      "close64" => Body::Close(text64()?),
      "binary" => Body::Binary(unbase64()?),
      _ => return Err(DecodeError::Unknown(kind.into())),
    };
    Ok(Self {
      recv,
      inst: inst.into(),
      conn,
      body,
    })
  }
}
