//! The spacing and type scale the screens draw from, so headers, rows and labels line up
//! from one page to the next. Colors stay in theme.rs; this is only sizes.

use super::gfx::Font;

// Spacing, in DIPs. Every gap on a page is one of these.
pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
pub const S4: f32 = 16.0;
pub const S5: f32 = 24.0;

// Corner radii: small controls, rows and fields, cards.
pub const R_SM: f32 = 6.0;
pub const R_MD: f32 = 8.0;
pub const R_LG: f32 = 10.0;

// Type. Serif for what names a page, sans for everything you act on.
pub const TITLE: Font = Font::serif(30.0);
pub const H2: Font = Font::serif(22.0);
pub const BIG_NUM: Font = Font::serif(28.0);
pub const BODY: Font = Font::ui(13.0, 450);
pub const BODY_STRONG: Font = Font::ui(13.0, 550);
pub const SMALL: Font = Font::ui(12.0, 400);
pub const SMALL_STRONG: Font = Font::ui(12.0, 500);
pub const CAPTION: Font = Font::ui(11.0, 600);

/// Page rows: a list row, and the table row of the Control page.
pub const ROW_H: f32 = 40.0;
/// Width of the content column on list pages, and its minimum side margin (the close button lives in it).
pub const COL_W: f32 = 760.0;
pub const COL_MARGIN: f32 = 56.0;
