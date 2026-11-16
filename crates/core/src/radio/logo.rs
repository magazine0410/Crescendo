//! Station logos, fetched by the backend and cached as small PNGs.
//!
//! A logo's address comes from the directory and points at the station's own
//! website, so it is fetched only through the plugin network guard's resolver:
//! public addresses only, no proxy, bounded size and time. The webview never
//! sees the address. It asks for `crescendo-art://…/logo/{key}`, and only keys
//! this store registered (from a saved station or a listing it returned) are
//! served.

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LOGO_BYTES: u64 = 2 * 1024 * 1024;
const LOGO_SIZE: u32 = 128;
/// How long a logo that could not be fetched is not tried again.
const RETRY_AFTER: Duration = Duration::from_secs(24 * 3600);

/// The cache key for a logo address.
pub fn key_for(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

pub struct LogoStore {
    dir: PathBuf,
    known: Mutex<HashMap<String, String>>,
}

impl LogoStore {
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let dir = cache_dir.join("station-logos");
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            known: Mutex::new(HashMap::new()),
        })
    }

    /// Allow the logo at `url` to be served, and give its key.
    pub fn register(&self, url: &str) -> String {
        let key = key_for(url);
        self.known
            .lock()
            .unwrap()
            .insert(key.clone(), url.to_string());
        key
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.png"))
    }

    fn miss_marker(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.none"))
    }

    /// The cached file for a key, if it has been fetched already.
    pub fn cached(&self, key: &str) -> Option<PathBuf> {
        let path = self.path(key);
        path.is_file().then_some(path)
    }

    /// The logo for a registered key, fetching it on first use. None when
    /// the key is unknown or the logo could not be had.
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        if !key.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        if let Some(path) = self.cached(key) {
            return Some(path);
        }
        let marker = self.miss_marker(key);
        let recent_miss = std::fs::metadata(&marker)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < RETRY_AFTER);
        if recent_miss {
            return None;
        }
        let url = self.known.lock().unwrap().get(key).cloned()?;
        match fetch_thumbnail(&url) {
            Ok(png) => {
                let path = self.path(key);
                let tmp = self.dir.join(format!("{key}.tmp"));
                std::fs::write(&tmp, png).ok()?;
                std::fs::rename(&tmp, &path).ok()?;
                let _ = std::fs::remove_file(&marker);
                Some(path)
            }
            Err(error) => {
                log::info!("station logo {url} unavailable: {error:#}");
                let _ = std::fs::write(&marker, b"");
                None
            }
        }
    }
}

fn fetch_thumbnail(url: &str) -> Result<Vec<u8>> {
    super::check_stream_url(url)?;
    let agent = crate::plugins::net::public_only_agent(FETCH_TIMEOUT);
    let mut response = agent.get(url).call()?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_LOGO_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LOGO_BYTES {
        bail!("the logo is larger than 2 MiB");
    }
    thumbnail(&bytes)
}

/// Decode an image and fit it into a PNG of at most LOGO_SIZE square.
fn thumbnail(bytes: &[u8]) -> Result<Vec<u8>> {
    let image = image::load_from_memory(bytes)?;
    let image = if image.width() > LOGO_SIZE || image.height() > LOGO_SIZE {
        image.thumbnail(LOGO_SIZE, LOGO_SIZE)
    } else {
        image
    };
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_registered_hex_keys_are_served() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogoStore::new(dir.path()).unwrap();
        assert_eq!(store.get("../../etc/passwd"), None);
        assert_eq!(store.get(&key_for("https://unregistered.example/a.png")), None);
        let key = store.register("https://x.example/logo.png");
        assert_eq!(key, key_for("https://x.example/logo.png"));
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn a_logo_on_a_private_address_is_refused_and_remembered() {
        // A server that would answer, so only the guard can keep it from being
        // reached.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let store = LogoStore::new(dir.path()).unwrap();
        let key = store.register(&format!("http://127.0.0.1:{port}/logo.png"));
        assert_eq!(store.get(&key), None);
        assert!(store.miss_marker(&key).exists());
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "nothing may connect to a loopback logo"
        );
        // Remembered: a second ask does not try again.
        assert_eq!(store.get(&key), None);
    }

    #[test]
    fn large_images_shrink_to_a_small_png() {
        let image = image::RgbImage::from_pixel(512, 256, image::Rgb([200, 30, 30]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let small = image::load_from_memory(&thumbnail(png.get_ref()).unwrap()).unwrap();
        assert_eq!((small.width(), small.height()), (128, 64));
    }
}
