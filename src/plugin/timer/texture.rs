//! Text-to-texture for the in-game timer overlay.
//!
//! Ported from `hud/labels/texture.rs` with a larger default font size
//! (24 pt for the clock line; callers may request 16 pt for split rows).

use std::{cell::RefCell, mem};

use classicube_sys::{
    Context2D_DrawText, DrawTextArgs, Drawer2D_TextHeight, Drawer2D_TextWidth,
    FONT_FLAGS_FONT_FLAGS_NONE, Font_Free, Font_Make, FontDesc, Gfx, OwnedContext2D, OwnedString,
    OwnedTexture, TextureRec,
};

const CLOCK_FONT_SIZE: i32 = 24;
const SPLIT_FONT_SIZE: i32 = 16;

thread_local! {
    static CLOCK_FONT: RefCell<Option<FontDesc>> = const { RefCell::new(None) };
    static SPLIT_FONT: RefCell<Option<FontDesc>> = const { RefCell::new(None) };
}

fn with_font<R>(
    size: i32,
    slot: &'static std::thread::LocalKey<RefCell<Option<FontDesc>>>,
    f: impl FnOnce(&mut FontDesc) -> R,
) -> R {
    slot.with_borrow_mut(|slot| {
        let font = slot.get_or_insert_with(|| unsafe {
            let mut font = mem::zeroed();
            Font_Make(&mut font, size, FONT_FLAGS_FONT_FLAGS_NONE as _);
            font
        });
        f(font)
    })
}

fn free_font(slot: &'static std::thread::LocalKey<RefCell<Option<FontDesc>>>) {
    slot.with_borrow_mut(|slot| {
        if let Some(mut font) = slot.take() {
            unsafe { Font_Free(&mut font) };
        }
    });
}

pub fn free() {
    free_font(&CLOCK_FONT);
    free_font(&SPLIT_FONT);
}

/// Render `text` to a GPU texture at the clock font size (24 pt).
pub fn create_clock_texture(text: &str) -> Option<OwnedTexture> {
    create_texture(text, CLOCK_FONT_SIZE, &CLOCK_FONT)
}

/// Render `text` to a GPU texture at the split-row font size (16 pt).
pub fn create_split_texture(text: &str) -> Option<OwnedTexture> {
    create_texture(text, SPLIT_FONT_SIZE, &SPLIT_FONT)
}

fn create_texture(
    text: &str,
    size: i32,
    slot: &'static std::thread::LocalKey<RefCell<Option<FontDesc>>>,
) -> Option<OwnedTexture> {
    if unsafe { Gfx.LostContext } != 0 {
        return None;
    }

    let owned = OwnedString::new(text);
    with_font(size, slot, |font| unsafe {
        let mut args = DrawTextArgs {
            text: owned.get_cc_string(),
            font,
            useShadow: 1,
        };
        let width = Drawer2D_TextWidth(&mut args);
        if width == 0 {
            return None;
        }
        let height = Drawer2D_TextHeight(&mut args);
        let w = u16::try_from(width).ok()?;
        let h = u16::try_from(height).ok()?;

        let mut context = OwnedContext2D::new_pow_of_2(width, height, 0);
        Context2D_DrawText(context.as_context_2d_mut(), &mut args, 0, 0);

        let bmp_w = u16::try_from(context.as_bitmap().width).ok()?;
        let bmp_h = u16::try_from(context.as_bitmap().height).ok()?;
        let uv = TextureRec {
            u1: 0.0,
            v1: 0.0,
            u2: f32::from(w) / f32::from(bmp_w),
            v2: f32::from(h) / f32::from(bmp_h),
        };
        OwnedTexture::new(context.as_bitmap_mut(), (0, 0), (w, h), uv)
    })
}
