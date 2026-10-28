use image::ImageFormat;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15; // 文件/文件夹拖拽格式

pub static IS_WRITING_CLIPBOARD: AtomicBool = AtomicBool::new(false);
static RECENT_HASHES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn mark_hash_processed(hash: &str) {
    if hash.is_empty() {
        return;
    }
    let mut list = RECENT_HASHES.lock().unwrap();
    let upper = hash.to_uppercase();
    if !list.contains(&upper) {
        if list.len() >= 32 {
            list.remove(0);
        }
        list.push(upper);
    }
}

pub fn is_hash_processed(hash: &str) -> bool {
    if hash.is_empty() {
        return false;
    }
    let list = RECENT_HASHES.lock().unwrap();
    list.iter().any(|h| h.eq_ignore_ascii_case(hash))
}

#[derive(Debug, Clone)]
pub enum ClipboardContent {
    Text(String),
    Image(Vec<u8>), // PNG 格式字节流
}

impl ClipboardContent {
    pub fn compute_sha256(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    pub fn compute_hash(&self) -> String {
        match self {
            ClipboardContent::Text(t) => Self::compute_sha256(t.as_bytes()),
            ClipboardContent::Image(bytes) => Self::compute_sha256(bytes),
        }
    }
}

pub struct ClipboardManager;

impl ClipboardManager {
    unsafe fn open_clipboard_with_retry() -> bool {
        for _ in 0..6 {
            if OpenClipboard(None).is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    pub fn read_clipboard() -> Option<ClipboardContent> {
        unsafe {
            if !Self::open_clipboard_with_retry() {
                return None;
            }

            let result = Self::read_clipboard_inner();
            let _ = CloseClipboard();
            result
        }
    }

    unsafe fn read_clipboard_inner() -> Option<ClipboardContent> {
        // 0. 优先检测是否为文件/文件夹复制 (CF_HDROP)，若是则直接忽略，绝不上传
        if IsClipboardFormatAvailable(CF_HDROP).is_ok() {
            return None;
        }

        // 1. 优先尝试读取图片 (CF_DIB)
        let dib_handle = GetClipboardData(CF_DIB);
        if let Ok(handle) = dib_handle {
            if !handle.0.is_null() {
                let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(handle.0));
                if !ptr.is_null() {
                    let size = GlobalSize(windows::Win32::Foundation::HGLOBAL(handle.0));
                    let dib_slice = std::slice::from_raw_parts(ptr as *const u8, size);
                    let png_result = dib_to_png(dib_slice);
                    let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(handle.0));

                    if let Ok(png_bytes) = png_result {
                        return Some(ClipboardContent::Image(png_bytes));
                    }
                }
            }
        }

        // 2. 尝试读取文本 (CF_UNICODETEXT)
        let text_handle = GetClipboardData(CF_UNICODETEXT);
        if let Ok(handle) = text_handle {
            if !handle.0.is_null() {
                let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(handle.0));
                if !ptr.is_null() {
                    let u16_ptr = ptr as *const u16;
                    let mut len = 0;
                    while *u16_ptr.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(u16_ptr, len);
                    let text = String::from_utf16_lossy(slice);
                    let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(handle.0));
                    return Some(ClipboardContent::Text(text));
                }
            }
        }

        None
    }

    pub fn write_text(text: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        IS_WRITING_CLIPBOARD.store(true, Ordering::SeqCst);
        let res = unsafe {
            if !Self::open_clipboard_with_retry() {
                IS_WRITING_CLIPBOARD.store(false, Ordering::SeqCst);
                return Err("无法打开剪贴板".into());
            }

            let _ = EmptyClipboard();

            let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes_len = utf16.len() * 2;
            let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes_len)?;
            let ptr = GlobalLock(hmem);
            if ptr.is_null() {
                let _ = CloseClipboard();
                IS_WRITING_CLIPBOARD.store(false, Ordering::SeqCst);
                return Err("GlobalLock 失败".into());
            }

            std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, bytes_len);
            let _ = GlobalUnlock(hmem);

            let set_res = SetClipboardData(CF_UNICODETEXT, HANDLE(hmem.0));
            let _ = CloseClipboard();

            if set_res.is_err() {
                return Err("SetClipboardData 失败".into());
            }
            Ok(())
        };
        IS_WRITING_CLIPBOARD.store(false, Ordering::SeqCst);
        res
    }

    pub fn write_image(png_bytes: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        IS_WRITING_CLIPBOARD.store(true, Ordering::SeqCst);
        let res = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let dib_bytes = png_to_dib(png_bytes)?;
            unsafe {
                if !Self::open_clipboard_with_retry() {
                    return Err("无法打开剪贴板".into());
                }

                let _ = EmptyClipboard();

                let hmem = GlobalAlloc(GMEM_MOVEABLE, dib_bytes.len())?;
                let ptr = GlobalLock(hmem);
                if ptr.is_null() {
                    let _ = CloseClipboard();
                    return Err("GlobalLock 失败".into());
                }

                std::ptr::copy_nonoverlapping(dib_bytes.as_ptr(), ptr as *mut u8, dib_bytes.len());
                let _ = GlobalUnlock(hmem);

                let set_res = SetClipboardData(CF_DIB, HANDLE(hmem.0));
                let _ = CloseClipboard();

                if set_res.is_err() {
                    return Err("SetClipboardData 写入图片失败".into());
                }
                Ok(())
            }
        })();
        IS_WRITING_CLIPBOARD.store(false, Ordering::SeqCst);
        res
    }
}

// 将 Windows DIB 字节数据组装成 BMP 并转码为 PNG
fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    if dib.len() < std::mem::size_of::<BITMAPINFOHEADER>() {
        return Err("DIB 数据长度过短".into());
    }

    let header_size = u32::from_le_bytes(dib[0..4].try_into()?) as usize;
    let bit_count = u16::from_le_bytes(dib[14..16].try_into()?);
    let clr_used = u32::from_le_bytes(dib[32..36].try_into()?) as usize;

    let palette_size = if clr_used > 0 {
        clr_used * 4
    } else if bit_count <= 8 {
        (1usize << bit_count) * 4
    } else {
        0
    };

    let offset_bits = 14 + header_size + palette_size;
    let total_file_size = 14 + dib.len();

    let mut bmp_data = Vec::with_capacity(total_file_size);
    // 写入 BITMAPFILEHEADER
    bmp_data.extend_from_slice(b"BM");
    bmp_data.extend_from_slice(&(total_file_size as u32).to_le_bytes());
    bmp_data.extend_from_slice(&[0u8; 4]); // 保留字节
    bmp_data.extend_from_slice(&(offset_bits as u32).to_le_bytes());
    // 写入 DIB 数据
    bmp_data.extend_from_slice(dib);

    let img = image::load_from_memory_with_format(&bmp_data, ImageFormat::Bmp)?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)?;
    Ok(png_bytes)
}

// 将 PNG 字节流解码并转为 Windows DIB
fn png_to_dib(png: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let img = image::load_from_memory_with_format(png, ImageFormat::Png)?;
    let mut bmp_buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut bmp_buf), ImageFormat::Bmp)?;

    // 跳过 BMP 的 14 字节 BITMAPFILEHEADER，剩余即为 DIB
    if bmp_buf.len() <= 14 {
        return Err("BMP 数据异常".into());
    }
    Ok(bmp_buf[14..].to_vec())
}
