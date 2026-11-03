use crate::utils::error::AppError;

pub fn validate_signature(bytes: &[u8]) -> Result<(), AppError> {
    if bytes.len() < 4 {
        return Err(AppError::Validation("file too small".into()));
    }
    // Images
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok(());
    } // JPEG
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Ok(());
    } // PNG
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok(());
    } // GIF
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok(());
    } // WEBP
    if bytes.starts_with(b"BM") {
        return Ok(());
    } // BMP
    // Videos
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Ok(());
    } // MP4/MOV
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Ok(());
    } // WebM/MKV
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"AVI " {
        return Ok(());
    } // AVI
    if bytes.starts_with(b"\x00\x00\x01\xB3") {
        return Ok(());
    } // MPEG
    // Allow unknown if content-type already validated? Be strict but allow octet-stream-ish
    Err(AppError::Validation("unknown file signature".into()))
}

pub fn extension_from_content_type(ct: &str) -> Option<String> {
    let ct = ct.split(';').next().unwrap_or(ct).trim().to_lowercase();
    match ct.as_str() {
        "image/jpeg" => Some("jpg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        "image/bmp" => Some("bmp".to_string()),
        "video/mp4" => Some("mp4".to_string()),
        "video/webm" => Some("webm".to_string()),
        "video/x-msvideo" | "video/avi" => Some("avi".to_string()),
        "video/quicktime" => Some("mov".to_string()),
        "video/x-matroska" => Some("mkv".to_string()),
        "video/mpeg" => Some("mpg".to_string()),
        "application/octet-stream" => None,
        _ => None,
    }
}

pub fn extension_from_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg".to_string());
    }
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some("png".to_string());
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif".to_string());
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp".to_string());
    }
    if bytes.starts_with(b"BM") {
        return Some("bmp".to_string());
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some("mp4".to_string());
    }
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("webm".to_string());
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"AVI " {
        return Some("avi".to_string());
    }
    None
}
