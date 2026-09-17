//! System cursor theme loading for the visible pointer.
//!
//! Mirrors Smithay anvil's cursor handling structurally (theme lookup,
//! nominal-size selection, RGBA upload format): solved plumbing, not novel
//! architecture. Falls back to a generated square when no theme is usable,
//! so the pointer is always visible.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use tracing::{info, warn};

/// A cursor image ready to hand to the renderer, with its hotspot in pixels.
pub struct ThemeCursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: (i32, i32),
}

/// Load the `default` cursor at XCURSOR_SIZE (or 24px), else a fallback.
pub fn load_default_cursor() -> ThemeCursor {
    let name =
        std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".to_string());
    let size: u32 = std::env::var("XCURSOR_SIZE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(24);

    match load_theme_cursor(&name, size) {
        Some(cursor) => {
            info!("Loaded system cursor theme");
            cursor
        }
        None => {
            warn!("No usable system cursor; using built-in fallback square");
            fallback_cursor()
        }
    }
}

fn load_theme_cursor(name: &str, size: u32) -> Option<ThemeCursor> {
    let theme = xcursor::CursorTheme::load(name);
    let path = theme.load_icon("default")?;
    let bytes = std::fs::read(&path).ok()?;
    let images = xcursor::parser::parse_xcursor(&bytes)?;
    let best = images
        .iter()
        .min_by_key(|image| size.abs_diff(image.size))?;
    info!(
        path = ?path,
        width = best.width,
        height = best.height,
        "Using cursor theme image"
    );
    Some(ThemeCursor {
        buffer: MemoryRenderBuffer::from_slice(
            &best.pixels_rgba,
            Fourcc::Abgr8888,
            (best.width as i32, best.height as i32),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: (best.xhot as i32, best.yhot as i32),
    })
}

/// Fully opaque white square. Ugly, unmissable, always available.
fn fallback_cursor() -> ThemeCursor {
    const SIDE: i32 = 16;
    let pixels = vec![255u8; (SIDE * SIDE * 4) as usize];
    ThemeCursor {
        buffer: MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Abgr8888,
            (SIDE, SIDE),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: (0, 0),
    }
}
