//! macOS client image clipboard exchange; the remote capability is authoritative.
use maho_app::ClientSession;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard};
use objc2_foundation::{NSData, NSDictionary, NSString};
use std::time::{Duration, Instant};

const LIMIT: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub struct ImageClipboard {
    change_count: Option<isize>,
    next_poll: Option<Instant>,
}

impl ImageClipboard {
    fn local_change(&mut self, count: isize) -> bool {
        self.change_count.replace(count) != Some(count)
    }
    pub fn reset(&mut self) {
        self.change_count = Some(NSPasteboard::generalPasteboard().changeCount());
        self.next_poll = None;
    }

    pub fn exchange(&mut self, session: &ClientSession, enabled: bool) -> Result<(), String> {
        // Drain even when disabled so an earlier transfer cannot appear after opt-in.
        let incoming = session.take_clipboard_image().map_err(|e| e.to_string())?;
        let pasteboard = NSPasteboard::generalPasteboard();
        if !enabled
            || session
                .effective_permissions()
                .map_err(|e| e.to_string())?
                .is_some_and(|p| !p.clipboard)
        {
            self.change_count = Some(pasteboard.changeCount());
            return Ok(());
        }
        if let Some(bytes) = incoming {
            validate_png(&bytes)?;
            // Poll and remote write share the ImageClipboard lock. Record only
            // after writing, so the next local poll suppresses the remote echo.
            let data = NSData::with_bytes(&bytes);
            pasteboard.clearContents();
            if !pasteboard.setData_forType(Some(&data), &NSString::from_str("public.png")) {
                return Err("Pasteboard rejected PNG write".into());
            }
            self.change_count = Some(pasteboard.changeCount());
        }
        let now = Instant::now();
        if self.next_poll.is_some_and(|next| now < next) {
            return Ok(());
        }
        self.next_poll = Some(now + Duration::from_millis(500));
        let count = pasteboard.changeCount();
        if !self.local_change(count) {
            return Ok(());
        }
        for ty in maho_app::CONCEALED_CLIPBOARD_TYPES {
            if pasteboard.dataForType(&NSString::from_str(ty)).is_some() {
                return Ok(());
            }
        }
        let png = pasteboard.dataForType(&NSString::from_str("public.png"));
        let bytes = if let Some(data) = png {
            if data.length() > LIMIT {
                return Err("Clipboard PNG exceeds 8 MiB".into());
            }
            data.to_vec()
        } else if let Some(data) = pasteboard.dataForType(&NSString::from_str("public.tiff")) {
            if data.length() > LIMIT {
                return Err("Clipboard TIFF exceeds 8 MiB".into());
            }
            let bitmap =
                NSBitmapImageRep::imageRepWithData(&data).ok_or("Invalid clipboard TIFF")?;
            if bitmap.pixelsWide() <= 0
                || bitmap.pixelsHigh() <= 0
                || (bitmap.pixelsWide() as u64).saturating_mul(bitmap.pixelsHigh() as u64)
                    > 16 * 1024 * 1024
            {
                return Err("Clipboard image exceeds decoded size limit".into());
            }
            let png = unsafe {
                bitmap.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }
            .ok_or("TIFF to PNG conversion failed")?;
            if png.length() > LIMIT {
                return Err("Converted PNG exceeds 8 MiB".into());
            }
            png.to_vec()
        } else {
            return Ok(());
        };
        validate_png(&bytes)?;
        session
            .send_clipboard_image(&bytes)
            .map_err(|e| e.to_string())
    }
}

fn validate_png(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > LIMIT || !bytes.starts_with(maho_proto::PNG_SIGNATURE) {
        return Err("Invalid or oversized clipboard PNG".into());
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    reader.limits(limits);
    reader.decode().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_and_oversized_png() {
        assert!(validate_png(maho_proto::PNG_SIGNATURE).is_err());
        assert!(validate_png(&vec![0; LIMIT + 1]).is_err());
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert!(validate_png(bytes.get_ref()).is_ok());
    }

    #[test]
    fn remote_write_count_suppresses_echo_until_local_copy() {
        let mut clipboard = ImageClipboard {
            change_count: Some(12),
            next_poll: None,
        };
        assert!(!clipboard.local_change(12));
        assert!(clipboard.local_change(13));
        assert!(!clipboard.local_change(13));
    }
}
