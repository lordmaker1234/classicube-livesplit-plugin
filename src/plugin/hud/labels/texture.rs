//! Text-to-texture rendering for the floating checkpoint labels.
//!
//! Adapted (and trimmed) from the bubble texture builder in
//! `classicube-chat-bubbles-plugin/src/plugin/rendering/bubble/helpers.rs`:
//! we keep the lazily-built thread-local `FontDesc` and the `Gfx.LostContext`
//! guard, but drop the bubble border (the nine-slice PNG parts) and the
//! front/back pair -- a label is just white text with a drop shadow on a
//! transparent canvas.

use std::{cell::RefCell, mem};

use classicube_sys::{
    Context2D_DrawText, DrawTextArgs, Drawer2D_TextHeight, Drawer2D_TextWidth,
    FONT_FLAGS_FONT_FLAGS_NONE, Font_Free, Font_Make, FontDesc, Gfx, OwnedContext2D, OwnedString,
    OwnedTexture, TextureRec,
};

/// Point size for the label font. Large enough to stay legible when the
/// billboard shrinks with distance; the world-space height is normalized
/// separately (see `LABEL_LINE_WORLD_HEIGHT` in the parent module).
const FONT_SIZE: i32 = 16;

thread_local! {
    /// Lazily-built font, reused across every label texture and freed in
    /// [`free`]. Building a `FontDesc` per label would be wasteful, and
    /// `Font_Make` must be paired with `Font_Free` to release the backing
    /// system-font handle.
    static FONT: RefCell<Option<FontDesc>> = const { RefCell::new(None) };
}

fn with_font<R>(f: impl FnOnce(&mut FontDesc) -> R) -> R {
    FONT.with_borrow_mut(|slot| {
        let font = slot.get_or_insert_with(|| unsafe {
            let mut font = mem::zeroed();
            Font_Make(&mut font, FONT_SIZE, FONT_FLAGS_FONT_FLAGS_NONE as _);
            font
        });
        f(font)
    })
}

/// Free the cached font handle. Called from the label layer's teardown so
/// a plugin reload doesn't leak the system-font allocation.
pub(super) fn free() {
    FONT.with_borrow_mut(|slot| {
        if let Some(mut font) = slot.take() {
            unsafe { Font_Free(&mut font) };
        }
    });
}

/// Render `label` to a GPU texture, returning `None` when nothing can be
/// drawn right now:
/// - the GPU context is lost (mid-device-reset on Windows D3D9) -- callers
///   leave the cache untouched and retry once the context is back;
/// - the label measures to zero width (empty / whitespace-only) -- there's
///   no text to show;
/// - the bitmap dimensions don't fit a `u16` or the GPU rejects the upload
///   (`OwnedTexture::new` returns `None`).
///
/// Color codes in the label render natively via `Drawer2D`; `useShadow`
/// adds a drop shadow so white text stays legible against any backdrop.
pub(super) fn create_label_texture(label: &str) -> Option<OwnedTexture> {
    if unsafe { Gfx.LostContext } != 0 {
        return None;
    }

    let owned = OwnedString::new(label);
    with_font(|font| unsafe {
        let mut args = DrawTextArgs {
            text: owned.get_cc_string(),
            font,
            useShadow: 1,
        };

        // `Drawer2D_TextWidth` / `Drawer2D_TextHeight` already fold in the
        // shadow offset when `useShadow` is set, so the measured size is the
        // exact canvas the text + shadow needs -- no extra padding.
        let width = Drawer2D_TextWidth(&mut args);
        if width == 0 {
            return None;
        }
        let height = Drawer2D_TextHeight(&mut args);

        let w = u16::try_from(width).ok()?;
        let h = u16::try_from(height).ok()?;

        // Transparent (alpha 0) canvas, rounded up to a power-of-two for the
        // GPU; the used region is mapped back via the UV rect below.
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

        // The billboard renderer (`Particle_DoRender`) reads only the
        // texture's size/uv and the world anchor, never its `x`/`y` screen
        // position, so origin is fine here.
        OwnedTexture::new(context.as_bitmap_mut(), (0, 0), (w, h), uv)
    })
}
