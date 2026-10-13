//! An article's cover photograph. What the school uploads is decoded, turned upright, bounded
//! and written again as a JPEG, so no camera metadata (place, device, time) reaches a family,
//! and kept in the object store this deployment already runs.
use image::{codecs::jpeg::JpegEncoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use std::{io::Cursor, sync::OnceLock, time::Duration};
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// What an upload may weigh, and the formats it may be.
pub const UPLOAD_MAX_BYTES: usize = 8 * 1024 * 1024;
pub const UPLOAD_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];
/// The stored photograph: at most this many pixels on its longer side.
const STORED_MAX_SIDE: u32 = 1608;
const STORED_QUALITY: u8 = 82;
const STORED_MAX_BYTES: usize = 4 * 1024 * 1024;
const SOURCE_MAX_SIDE: u32 = 12_000;
const SOURCE_MIN_SIDE: u32 = 16;

/// The photograph as it is stored, or `None` when the bytes are not a JPEG, PNG or WebP picture
/// of a reasonable size. Runs on the caller's thread: call it from a blocking task.
pub fn normalize(source: &[u8]) -> Option<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .ok()?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    ) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(SOURCE_MAX_SIDE);
    limits.max_image_height = Some(SOURCE_MAX_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    if picture.width() < SOURCE_MIN_SIDE || picture.height() < SOURCE_MIN_SIDE {
        return None;
    }
    if picture.width() > STORED_MAX_SIDE || picture.height() > STORED_MAX_SIDE {
        picture = picture.resize(
            STORED_MAX_SIDE,
            STORED_MAX_SIDE,
            image::imageops::FilterType::Triangle,
        );
    }
    // A JPEG has no transparency: what was see-through becomes white.
    let mut flat = image::RgbImage::new(picture.width(), picture.height());
    for (x, y, pixel) in picture.to_rgba8().enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        let over = |c: u8| ((u32::from(c) * alpha + 255 * (255 - alpha)) / 255) as u8;
        flat.put_pixel(x, y, image::Rgb([over(pixel[0]), over(pixel[1]), over(pixel[2])]));
    }
    let mut stored = Vec::new();
    JpegEncoder::new_with_quality(&mut stored, STORED_QUALITY)
        .encode_image(&flat)
        .ok()?;
    (stored.len() <= STORED_MAX_BYTES).then_some(stored)
}

/// Where a cover lives in the bucket this service shares: its own prefix, one object per upload.
pub fn object_key(tenant: &str, school: &str, cover: &str) -> String {
    format!("sis/article-covers/{tenant}/{school}/{cover}.jpg")
}

pub enum Store {
    S3 {
        bucket: Bucket,
        credentials: Credentials,
        http: reqwest::Client,
    },
    #[cfg(test)]
    Memory(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>),
}
impl Store {
    /// The store named by this deployment's `MINIO_*` settings, or `None` when it names none.
    fn from_env() -> Option<Store> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let endpoint = reqwest::Url::parse(&var("MINIO_ENDPOINT")?).ok()?;
        if !matches!(endpoint.scheme(), "http" | "https") {
            return None;
        }
        let bucket = Bucket::new(
            endpoint,
            UrlStyle::Path,
            var("MINIO_BUCKET")?,
            var("MINIO_REGION").unwrap_or_else(|| "us-east-1".into()),
        )
        .ok()?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .ok()?;
        Some(Store::S3 {
            bucket,
            credentials: Credentials::new(var("MINIO_ACCESS_KEY")?, var("MINIO_SECRET_KEY")?),
            http,
        })
    }
    pub async fn put(&self, key: &str, body: Vec<u8>) -> Result<(), Error> {
        match self {
            Store::S3 {
                bucket,
                credentials,
                http,
            } => {
                let url = bucket
                    .put_object(Some(credentials), key)
                    .sign(Duration::from_secs(60));
                let response = http
                    .put(url)
                    .header(reqwest::header::CONTENT_TYPE, "image/jpeg")
                    .body(body)
                    .send()
                    .await?;
                if !response.status().is_success() {
                    return Err("cover storage write failed".into());
                }
                Ok(())
            }
            #[cfg(test)]
            Store::Memory(objects) => {
                objects.lock().unwrap().insert(key.to_string(), body);
                Ok(())
            }
        }
    }
    pub async fn get(&self, key: &str) -> Result<Vec<u8>, Error> {
        match self {
            Store::S3 {
                bucket,
                credentials,
                http,
            } => {
                let url = bucket
                    .get_object(Some(credentials), key)
                    .sign(Duration::from_secs(60));
                let mut response = http.get(url).send().await?;
                if !response.status().is_success() {
                    return Err("cover storage read failed".into());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    if bytes.len() + chunk.len() > STORED_MAX_BYTES {
                        return Err("stored cover exceeds its limit".into());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(bytes)
            }
            #[cfg(test)]
            Store::Memory(objects) => objects
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or_else(|| "cover missing".into()),
        }
    }
}
/// This process's store, read from the environment once.
pub fn store() -> Option<&'static Store> {
    static STORE: OnceLock<Option<Store>> = OnceLock::new();
    STORE.get_or_init(Store::from_env).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn encoded(picture: DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        picture.write_to(&mut Cursor::new(&mut bytes), format).unwrap();
        bytes
    }
    #[test]
    fn a_photograph_is_stored_as_a_bounded_jpeg() {
        let wide = DynamicImage::new_rgb8(3000, 1500);
        let stored = normalize(&encoded(wide, ImageFormat::Png)).unwrap();
        let read = image::load_from_memory(&stored).unwrap();
        assert_eq!(image::guess_format(&stored).unwrap(), ImageFormat::Jpeg);
        assert_eq!((read.width(), read.height()), (1608, 804));
        let small = DynamicImage::new_rgb8(402, 250);
        let stored = normalize(&encoded(small, ImageFormat::Jpeg)).unwrap();
        let read = image::load_from_memory(&stored).unwrap();
        assert_eq!((read.width(), read.height()), (402, 250));
    }
    #[test]
    fn transparency_becomes_white() {
        let clear = DynamicImage::new_rgba8(64, 64);
        let stored = normalize(&encoded(clear, ImageFormat::Png)).unwrap();
        let pixel = image::load_from_memory(&stored).unwrap().to_rgb8()[(32, 32)];
        assert!(pixel.0.iter().all(|c| *c > 250), "{pixel:?}");
    }
    #[test]
    fn what_is_not_a_reasonable_picture_is_refused() {
        assert!(normalize(b"").is_none());
        assert!(normalize(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_none());
        assert!(normalize(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").is_none());
        assert!(normalize(&encoded(DynamicImage::new_rgb8(8, 8), ImageFormat::Png)).is_none());
        let mut cut = encoded(DynamicImage::new_rgb8(64, 64), ImageFormat::Png);
        cut.truncate(40);
        assert!(normalize(&cut).is_none());
        // A header that claims an enormous picture is refused before anything is allocated.
        let mut huge = encoded(DynamicImage::new_rgb8(64, 64), ImageFormat::Png);
        huge[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        assert!(normalize(&huge).is_none());
    }
    #[tokio::test]
    #[ignore = "requires a disposable S3 store at SCHOOL_ARTICLES_TEST_S3=http://127.0.0.1:3224 (root user fixture/fixture-secret)"]
    async fn a_cover_survives_the_object_store() {
        let endpoint = std::env::var("SCHOOL_ARTICLES_TEST_S3").unwrap();
        assert_eq!(endpoint, "http://127.0.0.1:3224");
        for (name, value) in [
            ("MINIO_ENDPOINT", endpoint.as_str()),
            ("MINIO_BUCKET", "ds-documents"),
            ("MINIO_ACCESS_KEY", "fixture"),
            ("MINIO_SECRET_KEY", "fixture-secret"),
        ] {
            std::env::set_var(name, value);
        }
        let store = Store::from_env().unwrap();
        let Store::S3 {
            bucket,
            credentials,
            http,
        } = &store
        else {
            unreachable!()
        };
        let create = bucket.create_bucket(credentials).sign(Duration::from_secs(60));
        let _ = http.put(create).send().await.unwrap();
        let key = object_key("TEN 1", "SCH:1", &format!("{}", std::process::id()));
        let photograph = normalize(&encoded(DynamicImage::new_rgb8(402, 250), ImageFormat::Png)).unwrap();
        store.put(&key, photograph.clone()).await.unwrap();
        assert_eq!(store.get(&key).await.unwrap(), photograph);
        assert!(store.get(&format!("{key}.missing")).await.is_err());
    }
    #[test]
    fn a_cover_has_its_own_place_in_the_shared_bucket() {
        assert_eq!(
            object_key("TEN-1", "SCH-1", "c0ffee"),
            "sis/article-covers/TEN-1/SCH-1/c0ffee.jpg"
        );
    }
}
