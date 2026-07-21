//! Design tokens — the spacing/radius/type scale every widget should pull
//! from instead of scattering magic numbers through `widgets.rs`. Keeping
//! these in one place is what makes the "glass console" look consistent
//! instead of accidentally-close.

/// Spacing scale, in logical pixels. Use for padding and gaps between
/// elements — pick the nearest step rather than inventing an in-between value.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 20.0;
    pub const XL: f32 = 32.0;
}

/// Corner radius scale, in logical pixels.
pub mod radius {
    pub const SM: f32 = 4.0;
    pub const MD: f32 = 6.0;
    pub const LG: f32 = 8.0;
}

/// Type scale, in points. Named by role rather than size so a global bump
/// (e.g. for readability) only touches this module.
pub mod type_scale {
    /// Meter/matrix cell glyphs, the smallest legible label.
    pub const CAPTION: u16 = 9;
    /// Field labels, pills, buttons.
    pub const LABEL: u16 = 10;
    /// Default body text — strip names, values.
    pub const BODY: u16 = 11;
    /// Section headers within a tab.
    pub const TITLE: u16 = 14;
    /// The wordmark / top-level display size.
    pub const DISPLAY: u16 = 20;
}

/// Layout breakpoints for the responsive strip/bus card row.
pub mod layout {
    /// Never shrink a strip card narrower than this before falling back to
    /// horizontal scrolling.
    pub const STRIP_MIN: f32 = 130.0;
    /// Never grow a strip card wider than this — extra width should go to
    /// gaps, not oversized cards.
    pub const STRIP_MAX: f32 = 190.0;
}
