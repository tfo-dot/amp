use amp_api::{DynProvider, RawImage};
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;

pub struct ImageCache {
    cache_dir: PathBuf,
}

impl ImageCache {
    pub fn new() -> Self {
        let cache_dir = ProjectDirs::from("com", "amp", "AMP")
            .map(|proj_dirs| proj_dirs.cache_dir().join("thumbnails"))
            .unwrap_or_else(|| PathBuf::from("cache/thumbnails"));

        if !cache_dir.exists() {
            let _ = fs::create_dir_all(&cache_dir);
        }

        Self { cache_dir }
    }

    fn get_path(&self, id: &str) -> PathBuf {
        let safe_id = id
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect::<String>();
        self.cache_dir.join(format!("{}.jpg", safe_id))
    }

    pub fn get_image(&self, id: &str) -> Option<RawImage> {
        let path = self.get_path(id);

        let bytes = std::fs::read(path).ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        let rgba = img.to_rgba8();

        Some(RawImage {
            width: rgba.width(),
            height: rgba.height(),
            rgba8: rgba.into_raw(),
        })
    }

    pub fn save_image(&self, id: &str, raw: &RawImage) {
        let path = self.get_path(id);

        let _ = image::save_buffer_with_format(
            path,
            &raw.rgba8,
            raw.width,
            raw.height,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Jpeg,
        );
    }

    pub async fn get_or_fetch(&self, id: &str, client: DynProvider) -> Option<RawImage> {
        if let Some(cached) = self.get_image(&id) {
            Some(cached)
        } else {
            let res = client.get_item_image_buffer(&id).await.ok();
            if let Some(ref b) = res {
                self.save_image(&id, b);
            }
            res
        }
    }
}
