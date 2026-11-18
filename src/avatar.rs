use crate::ctx::AppState;
use crate::{db, util};
use image::{codecs::png::PngEncoder, ImageEncoder, ImageError, ImageFormat, ImageReader, Limits};
use js_sys::{Reflect, Uint8Array};
use serde::Deserialize;
use std::io::Cursor;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::ReadableStreamDefaultReader;
use worker::*;

const FILE_LIMIT: usize = 512 * 1024;
const FORM_LIMIT: u32 = FILE_LIMIT as u32 + 16 * 1024;
const DIMENSION_LIMIT: u32 = 1024;
const DECODE_LIMIT: u64 = 16 * 1024 * 1024;
const THUMBNAIL_EDGE: u32 = 128;

pub(crate) enum FormError {
    Invalid,
    TooLarge,
}

pub(crate) async fn read_form(req: &mut Request) -> std::result::Result<FormData, FormError> {
    let capacity = match util::header(req, "Content-Length") {
        Some(length) => {
            let length = length.parse::<u64>().map_err(|_| FormError::Invalid)?;
            if length > u64::from(FORM_LIMIT) {
                return Err(FormError::TooLarge);
            }
            length as u32
        }
        None => FORM_LIMIT,
    };
    let content_type = util::header(req, "Content-Type").ok_or(FormError::Invalid)?;
    if !content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("multipart/form-data"))
    {
        return Err(FormError::Invalid);
    }

    let stream = req.inner().body().ok_or(FormError::Invalid)?;
    let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    // Keep the capped buffer in JavaScript for the Workers multipart parser.
    let buffered = Uint8Array::new_with_length(capacity);
    let done = JsValue::from_str("done");
    let value = JsValue::from_str("value");
    let mut len = 0;
    loop {
        let result = JsFuture::from(reader.read()).await.map_err(|_| FormError::Invalid)?;
        if Reflect::get(&result, &done).ok().and_then(|value| value.as_bool()) == Some(true) {
            reader.release_lock();
            break;
        }
        let chunk = Reflect::get(&result, &value)
            .ok()
            .and_then(|value| value.dyn_into::<Uint8Array>().ok())
            .ok_or(FormError::Invalid)?;
        let chunk_len = chunk.length();
        if chunk_len > capacity - len {
            let _ = JsFuture::from(reader.cancel()).await;
            return Err(FormError::TooLarge);
        }
        buffered.set(&chunk, len);
        len += chunk_len;
    }

    let headers = Headers::new();
    headers.set("Content-Type", &content_type).map_err(|_| FormError::Invalid)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(buffered.subarray(0, len).into()));
    let url = req.url().map_err(|_| FormError::Invalid)?;
    let mut bounded = Request::new_with_init(url.as_str(), &init).map_err(|_| FormError::Invalid)?;
    bounded.form_data().await.map_err(|_| FormError::Invalid)
}

pub(crate) enum UploadError {
    Missing,
    Invalid,
    TooLarge,
    Dimensions,
}

pub(crate) struct AvatarImage(Vec<u8>);

impl AvatarImage {
    pub(crate) async fn from_form(form: &FormData) -> std::result::Result<Self, UploadError> {
        let Some(FormEntry::File(file)) = form.get("avatar") else {
            return Err(UploadError::Missing);
        };
        let size = file.size();
        if size == 0 {
            return Err(UploadError::Missing);
        }
        if size > FILE_LIMIT {
            return Err(UploadError::TooLarge);
        }
        let bytes = file.bytes().await.map_err(|_| UploadError::Invalid)?;
        let format = image::guess_format(&bytes).map_err(|_| UploadError::Invalid)?;
        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) {
            return Err(UploadError::Invalid);
        }
        if format == ImageFormat::WebP {
            check_webp_dimensions(&bytes)?;
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(DIMENSION_LIMIT);
        limits.max_image_height = Some(DIMENSION_LIMIT);
        limits.max_alloc = Some(DECODE_LIMIT);
        reader.limits(limits);
        let decoded = reader.decode().map_err(|error| match error {
            ImageError::Limits(_) => UploadError::Dimensions,
            _ => UploadError::Invalid,
        })?;
        let thumbnail = if decoded.width() > THUMBNAIL_EDGE || decoded.height() > THUMBNAIL_EDGE {
            decoded.thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE)
        } else {
            decoded
        };
        let pixels = thumbnail.into_rgba8();
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                pixels.as_raw(),
                pixels.width(),
                pixels.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|_| UploadError::Invalid)?;
        Ok(Self(png))
    }

    pub(crate) async fn save(self, state: &AppState, user_id: i64) -> Result<()> {
        let blob = Uint8Array::from(self.0.as_slice()).buffer();
        db::run(
            &state.db()?,
            "INSERT INTO user_avatars (user_id, data) VALUES (?, ?)
             ON CONFLICT(user_id) DO UPDATE SET data = excluded.data",
            &[util::js_int(user_id), blob.into()],
        )
        .await
    }
}

fn check_webp_dimensions(bytes: &[u8]) -> std::result::Result<(), UploadError> {
    let Some((header, chunks)) = bytes.split_first_chunk::<12>() else {
        return Err(UploadError::Invalid);
    };
    let [b'R', b'I', b'F', b'F', len0, len1, len2, len3, b'W', b'E', b'B', b'P'] = *header else {
        return Err(UploadError::Invalid);
    };
    let riff_len = u32::from_le_bytes([len0, len1, len2, len3]) as usize;
    if riff_len != chunks.len() + 4 {
        return Err(UploadError::Invalid);
    }
    check_webp_chunks(chunks, true)
}

// A lossy WebP frame has its own dimensions, separate from the container canvas.
// Check them before the decoder allocates its frame buffers.
fn check_webp_chunks(mut bytes: &[u8], allow_frames: bool) -> std::result::Result<(), UploadError> {
    while !bytes.is_empty() {
        let Some((header, remainder)) = bytes.split_first_chunk::<8>() else {
            return Err(UploadError::Invalid);
        };
        let [a, b, c, d, len0, len1, len2, len3] = *header;
        let length = u32::from_le_bytes([len0, len1, len2, len3]) as usize;
        let (data, rest) = remainder.split_at_checked(length).ok_or(UploadError::Invalid)?;
        match [a, b, c, d] {
            [b'V', b'P', b'8', b' '] => {
                let [_, _, _, _, _, _, w0, w1, h0, h1, ..] = data else {
                    return Err(UploadError::Invalid);
                };
                let width = u32::from(u16::from_le_bytes([*w0, *w1]) & 0x3fff);
                let height = u32::from(u16::from_le_bytes([*h0, *h1]) & 0x3fff);
                if width > DIMENSION_LIMIT || height > DIMENSION_LIMIT {
                    return Err(UploadError::Dimensions);
                }
            }
            [b'A', b'N', b'M', b'F'] if allow_frames => {
                let chunks = data.get(16..).ok_or(UploadError::Invalid)?;
                check_webp_chunks(chunks, false)?;
            }
            _ => {}
        }
        bytes = rest.get(length % 2..).ok_or(UploadError::Invalid)?;
    }
    Ok(())
}

pub(crate) async fn show(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    #[derive(Deserialize)]
    struct AvatarRow {
        data: Vec<u8>,
    }
    let Some(id) = ctx.param("id")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|id| *id > 0)
    else {
        return Ok(Response::builder().with_status(404).empty());
    };
    let avatar = db::first::<AvatarRow>(
        &ctx.data.db()?,
        "SELECT data FROM user_avatars WHERE user_id = ?",
        &[util::js_int(id)],
    )
    .await?;
    let Some(avatar) = avatar else {
        return Ok(Response::builder().with_status(404).empty());
    };
    Ok(Response::builder()
        .with_header("Content-Type", "image/png")?
        .fixed(avatar.data))
}
