// Emotes: draws dgg emotes in the terminal, animated, with whichever image
// protocol it supports (Kitty, Sixel, iTerm2), or Unicode half-blocks.
//
// Usage: cargo run -p viewer --example emotes -- [--rows N] [--halfblocks]
//          [prefix ...]
// One row tall unless --rows says. With no prefixes, a few still and
// animated ones. q or Esc quits.

use {
  ::dgg_api::assets::{DEFAULT_CDN, Emote, fetch},
  ::image::{
    AnimationDecoder, DynamicImage, ImageFormat,
    codecs::{gif::GifDecoder, webp::WebPDecoder},
    imageops::FilterType,
  },
  ::ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode},
    layout::{Rect, Size},
    text::Line,
  },
  ::ratatui_image::{Image, Resize, picker::Picker, protocol::Protocol},
  ::std::{
    env,
    error::Error,
    io::Cursor,
    time::{Duration, Instant},
  },
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Shown with no prefixes given: how many still, how many animated.
const STILL: usize = 8;
const ANIMATED: usize = 10;

struct Options {
  rows: u16,
  halfblocks: bool,
  prefixes: Vec<String>,
}

fn options() -> Result<Options> {
  let mut options = Options {
    rows: 1,
    halfblocks: false,
    prefixes: Vec::new(),
  };
  let mut args = env::args().skip(1);
  while let Some(arg) = args.next() {
    match arg.as_str() {
      "--rows" => {
        options.rows = args.next().ok_or("--rows needs a number")?.parse()?
      }
      "--halfblocks" => options.halfblocks = true,
      _ => options.prefixes.push(arg),
    }
  }
  Ok(options)
}

/// One emote, ready to draw: each frame, with how long it shows.
struct Drawn {
  prefix: String,
  frames: Vec<(Protocol, Duration)>,
  frame: usize,
  /// When the next frame shows; never, for a still.
  next: Option<Instant>,
}

#[::tokio::main]
async fn main() -> Result<()> {
  let options = options()?;
  let _ = ::rustls::crypto::ring::default_provider().install_default();
  let client = ::reqwest::Client::new();
  let emotes = fetch::emotes(&client, DEFAULT_CDN, None).await?.value;
  let chosen = choose(&emotes, &options.prefixes);

  let mut terminal = ::ratatui::init();
  let result = run(&mut terminal, &client, &chosen, &options).await;
  ::ratatui::restore();
  result
}

/// The emotes named, or a few still and animated ones.
fn choose<'a>(emotes: &'a [Emote], prefixes: &[String]) -> Vec<&'a Emote> {
  if !prefixes.is_empty() {
    return emotes
      .iter()
      .filter(|emote| prefixes.contains(&emote.prefix))
      .collect();
  }
  fn mime<'a>(emote: &&'a Emote) -> Option<&'a str> {
    emote.image().map(|image| image.mime.as_str())
  }
  let still = emotes.iter().filter(|e| mime(e) == Some("image/png"));
  let animated = emotes
    .iter()
    .filter(|e| matches!(mime(e), Some("image/gif" | "image/webp")));
  still.take(STILL).chain(animated.take(ANIMATED)).collect()
}

async fn run(
  terminal: &mut DefaultTerminal,
  client: &::reqwest::Client,
  emotes: &[&Emote],
  options: &Options,
) -> Result<()> {
  // Asks the terminal, so after entering the alternate screen.
  let mut picker = Picker::from_query_stdio()?;
  if options.halfblocks {
    picker = Picker::halfblocks();
  }
  let header = format!(
    "{:?}, font {}x{} px, {} rows: loading {} emotes...",
    picker.protocol_type(),
    picker.font_size().width,
    picker.font_size().height,
    options.rows,
    emotes.len()
  );
  terminal
    .draw(|frame| frame.render_widget(Line::raw(&header), frame.area()))?;

  let mut drawn = Vec::new();
  let mut skipped = Vec::new();
  for emote in emotes {
    match load(client, &picker, emote, options.rows).await {
      Ok(frames) => drawn.push(Drawn {
        prefix: emote.prefix.clone(),
        frames,
        frame: 0,
        next: Some(Instant::now()),
      }),
      Err(e) => skipped.push(format!("{}: {e}", emote.prefix)),
    }
  }
  let header = format!(
    "{:?}, font {}x{} px, {} rows. q quits.{}",
    picker.protocol_type(),
    picker.font_size().width,
    picker.font_size().height,
    options.rows,
    match skipped.is_empty() {
      true => String::new(),
      false => format!(" Skipped {}", skipped.join("; ")),
    }
  );

  loop {
    let now = Instant::now();
    for emote in &mut drawn {
      if emote.next.is_some_and(|next| now >= next) {
        emote.frame = (emote.frame + 1) % emote.frames.len();
        emote.next = match emote.frames.len() {
          1 => None,
          _ => Some(now + emote.frames[emote.frame].1),
        };
      }
    }
    terminal.draw(|frame| {
      let area = frame.area();
      frame.render_widget(Line::raw(&header), area);
      // Left to right, wrapping: each emote, its prefix under it.
      let (mut x, mut y) = (area.x, area.y + 2);
      for emote in &drawn {
        let (protocol, _) = &emote.frames[emote.frame];
        let size = protocol.size();
        let width = size.width.max(emote.prefix.len() as u16) + 2;
        if x + width > area.right() && x > area.x {
          (x, y) = (area.x, y + size.height + 2);
        }
        if y + size.height + 1 > area.bottom() {
          break;
        }
        let image = Rect::new(x, y, size.width, size.height);
        frame.render_widget(Image::new(protocol), image);
        let label = Rect::new(x, y + size.height, width - 1, 1);
        frame.render_widget(Line::raw(emote.prefix.as_str()), label);
        x += width;
      }
    })?;
    let next = drawn.iter().filter_map(|emote| emote.next).min();
    let wait = next.map_or(Duration::from_millis(250), |next| {
      next.saturating_duration_since(Instant::now())
    });
    if event::poll(wait.min(Duration::from_millis(250)))?
      && let Event::Key(key) = event::read()?
      && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
    {
      return Ok(());
    }
  }
}

/// Downloads an emote's picture and makes each frame drawable, `rows`
/// tall.
async fn load(
  client: &::reqwest::Client,
  picker: &Picker,
  emote: &Emote,
  rows: u16,
) -> Result<Vec<(Protocol, Duration)>> {
  let image = emote.image().ok_or("no picture")?;
  let bytes = client
    .get(&image.url)
    .send()
    .await?
    .error_for_status()?
    .bytes()
    .await?;
  let frames: Vec<(DynamicImage, Duration)> =
    match ImageFormat::from_mime_type(&image.mime) {
      Some(ImageFormat::Gif) => {
        animation(GifDecoder::new(Cursor::new(&bytes))?)?
      }
      Some(ImageFormat::WebP) => {
        let decoder = WebPDecoder::new(Cursor::new(&bytes))?;
        match decoder.has_animation() {
          true => animation(decoder)?,
          false => {
            let still = ::image::load_from_memory(&bytes)?;
            vec![(still, Duration::ZERO)]
          }
        }
      }
      Some(format) => {
        let still = ::image::load_from_memory_with_format(&bytes, format)?;
        vec![(still, Duration::ZERO)]
      }
      None => return Err(format!("can't decode {}", image.mime).into()),
    };

  // As wide as the picture is at `rows` tall, in cells.
  let font = picker.font_size();
  let (width, height) = (frames[0].0.width(), frames[0].0.height());
  let pixels_high = u32::from(rows) * u32::from(font.height);
  let pixels_wide = width * pixels_high / height.max(1);
  let columns = pixels_wide.div_ceil(u32::from(font.width).max(1)).max(1);
  let size = Size::new(columns as u16, rows);
  frames
    .into_iter()
    .map(|(image, delay)| {
      let resized =
        image.resize(pixels_wide.max(1), pixels_high, FilterType::Triangle);
      let protocol = picker.new_protocol(resized, size, Resize::Fit(None))?;
      Ok((protocol, delay))
    })
    .collect()
}

/// Each frame, whole, with its delay. As browsers do, delays of 10 ms or
/// less show for 100 ms.
fn animation<'a>(
  decoder: impl AnimationDecoder<'a>,
) -> Result<Vec<(DynamicImage, Duration)>> {
  decoder
    .into_frames()
    .map(|frame| {
      let frame = frame?;
      let (numerator, denominator) = frame.delay().numer_denom_ms();
      let delay =
        Duration::from_millis(u64::from(numerator)) / denominator.max(1);
      let delay = match delay <= Duration::from_millis(10) {
        true => Duration::from_millis(100),
        false => delay,
      };
      Ok((DynamicImage::ImageRgba8(frame.into_buffer()), delay))
    })
    .collect()
}
