//! Pure media classification shared by semantic analysis and rendering adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MediaType {
    pub kind: MediaKind,
    pub mime: String,
}

pub fn embed_media_type(target: &str, explicit_type: Option<&str>) -> Option<MediaType> {
    if let Some(explicit) = explicit_type {
        return from_mime(
            explicit
                .split(';')
                .next()?
                .trim()
                .to_ascii_lowercase()
                .as_str(),
        );
    }
    let url = if target.starts_with("//") {
        url::Url::parse(&format!("https:{target}")).ok()
    } else {
        url::Url::parse(target).ok()
    };
    let path = url.as_ref().map_or(target, |url| url.path());
    let extension = path
        .rsplit('/')
        .next()?
        .rsplit_once('.')?
        .1
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mpeg" | "mpg" => "video/mpeg",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        _ => return None,
    };
    from_mime(mime)
}

fn from_mime(mime: &str) -> Option<MediaType> {
    use MediaKind::*;
    let (kind, mime) = match mime {
        "image/png" => (Image, "image/png"),
        "image/jpeg" => (Image, "image/jpeg"),
        "image/gif" => (Image, "image/gif"),
        "image/webp" => (Image, "image/webp"),
        "image/svg+xml" => (Image, "image/svg+xml"),
        "image/avif" => (Image, "image/avif"),
        "image/bmp" => (Image, "image/bmp"),
        "image/tiff" => (Image, "image/tiff"),
        "image/x-icon" | "image/vnd.microsoft.icon" => (Image, "image/x-icon"),
        "video/mp4" => (Video, "video/mp4"),
        "video/webm" => (Video, "video/webm"),
        "video/ogg" => (Video, "video/ogg"),
        "video/quicktime" => (Video, "video/quicktime"),
        "video/x-matroska" => (Video, "video/x-matroska"),
        "video/mpeg" => (Video, "video/mpeg"),
        "audio/mpeg" => (Audio, "audio/mpeg"),
        "audio/mp4" => (Audio, "audio/mp4"),
        "audio/ogg" => (Audio, "audio/ogg"),
        "audio/wav" | "audio/x-wav" => (Audio, "audio/wav"),
        "audio/flac" => (Audio, "audio/flac"),
        "audio/aac" => (Audio, "audio/aac"),
        "audio/webm" => (Audio, "audio/webm"),
        _ => return None,
    };
    Some(MediaType {
        kind,
        mime: mime.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_type_is_authoritative_even_when_unknown() {
        assert_eq!(
            embed_media_type("a.png", Some(" Audio/OGG; codecs=opus "))
                .unwrap()
                .kind,
            MediaKind::Audio
        );
        for value in ["", "broken", "image/unknown", "application/pdf"] {
            assert_eq!(embed_media_type("a.png", Some(value)), None);
        }
    }
    #[test]
    fn extension_uses_url_path_only_and_is_case_insensitive() {
        for target in [
            "a.PNG",
            "https://x.test/a.PNG?file=.mp4#x",
            "//x.test/a.PNG?x#y",
        ] {
            assert_eq!(
                embed_media_type(target, None).unwrap().kind,
                MediaKind::Image
            );
        }
        for target in [
            "https://host.png",
            "https://host/a?x=.png",
            "a.png?raw",
            "a.png#raw",
            "manual.pdf",
            "unknown",
        ] {
            assert_eq!(embed_media_type(target, None), None, "{target}");
        }
        assert_eq!(
            embed_media_type("a.MP3", None).unwrap().kind,
            MediaKind::Audio
        );
        assert_eq!(
            embed_media_type("a.WebM", None).unwrap().kind,
            MediaKind::Video
        );
    }
}
