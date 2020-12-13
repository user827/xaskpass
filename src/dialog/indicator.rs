use std::cmp::{max, min};
use std::convert::TryFrom as _;
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use anyhow::anyhow;
use log::trace;
use tokio::time::{sleep, Instant, Sleep};

use super::Pattern;
use crate::config;
use crate::errors::Result;

enum BlinkBg<'a> {
    Background,
    Pattern(&'a Pattern),
}

#[derive(Debug)]
pub struct Base {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
    border_width: f64,
    has_focus: bool,
    foreground: Pattern,
    background: Pattern,
    border_pattern: Pattern,
    border_pattern_focused: Pattern,
    indicator_pattern: Pattern,
    pass_len: u32,
    pub(super) dirty: bool,
    pub(super) dirty_blink: bool,
    pub blink_do: bool,
    blink_enabled: bool,
    blink_on: bool,
    pub show_selection_do: bool,
}

impl Base {
    pub fn new(config: config::IndicatorCommon, height: f64) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height,
            border_width: config.border_width,
            foreground: config.foreground.into(),
            background: Pattern::get_pattern(
                height - config.border_width,
                config.background,
                config.background_stop,
            ),
            border_pattern: config.border_color.into(),
            border_pattern_focused: config.border_color_focused.into(),
            indicator_pattern: Pattern::get_pattern(
                height - config.border_width,
                config.indicator_color,
                config.indicator_color_stop,
            ),
            has_focus: false,
            pass_len: 0,
            dirty: false,
            dirty_blink: false,
            blink_on: config.blink,
            blink_do: config.blink,
            blink_enabled: config.blink,
            show_selection_do: false,
        }
    }

    fn clear(&mut self, cr: &cairo::Context, background: &super::Pattern) {
        // offset by one to clear antialiasing too
        cr.rectangle(
            self.x - 1.0,
            self.y - 1.0,
            self.width + 2.0,
            self.height + 2.0,
        );
        cr.set_source(background);
        cr.fill();
    }

    fn blink(&mut self, cr: &cairo::Context, height: f64, x: f64, y: f64, bg: BlinkBg) {
        cr.save();

        cr.translate(self.x, self.y);

        if self.has_focus && self.blink_on {
            cr.set_source(&self.foreground);
            cr.move_to(x, y);
            cr.rel_line_to(0.0, height);
            cr.set_line_width(1.0);
            cr.stroke();
        } else {
            cr.rectangle(x - 1.0, y - 1.0, 3.0, height + 2.0);
            let bg = if let BlinkBg::Pattern(pat) = bg {
                pat
            } else {
                &self.background
            };
            cr.set_source(bg);
            cr.fill();
        };

        cr.restore();

        self.dirty_blink = false;
    }

    pub fn on_show_selection_timeout(&mut self) -> bool {
        self.show_selection_do = false;
        self.dirty = true;
        true
    }
    pub fn show_selection(&mut self, pass_len: usize, show_selection_timeout: &mut Sleep) -> bool {
        if pass_len as u32 != self.pass_len {
            self.pass_len = pass_len as u32;
            self.show_selection_do = true;
            show_selection_timeout.reset(
                Instant::now()
                    .checked_add(Duration::from_millis(200))
                    .unwrap(),
            );
            self.dirty = true;
        }
        self.dirty
    }

    pub fn passphrase_updated(&mut self, len: usize) -> bool {
        if len as u32 != self.pass_len {
            self.dirty = true;
        }
        self.pass_len = len as u32;
        self.dirty
    }

    pub fn set_focused(&mut self, is_focused: bool, blink_timeout: &mut Sleep) -> bool {
        self.dirty = self.dirty || is_focused != self.has_focus;
        self.has_focus = is_focused;
        if self.blink_enabled {
            self.blink_on = is_focused;
            if is_focused {
                self.reset_blink(blink_timeout);
            }
            self.blink_do = is_focused;
        }
        self.dirty
    }

    pub fn on_blink_timeout(&mut self, blink_timeout: &mut Sleep) -> bool {
        trace!("blink timeout");
        self.blink_on = !self.blink_on;
        self.dirty_blink = true;
        self.reset_blink(blink_timeout);
        self.dirty_blink
    }

    pub fn init_blink(&mut self) -> Sleep {
        sleep(Duration::from_millis(800))
    }

    fn reset_blink(&mut self, blink_timeout: &mut Sleep) {
        let duration = if self.blink_on {
            Duration::from_millis(800)
        } else {
            Duration::from_millis(400)
        };
        blink_timeout.reset(Instant::now().checked_add(duration).unwrap());
    }
}

#[derive(Debug)]
pub struct Circle {
    base: Base,
    indicator_count: u32,
    inner_radius: f64,
    spacing_angle: f64,
    lock_color: Pattern,
}

impl Deref for Circle {
    type Target = Base;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl DerefMut for Circle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl Circle {
    pub fn new(
        config: config::IndicatorCommon,
        circle: config::IndicatorCircle,
        text_height: f64,
    ) -> Self {
        let diameter = circle.diameter.unwrap_or(text_height * 3.0);
        let inner_radius =
            (diameter / 2.0 - circle.indicator_width.unwrap_or(diameter / 4.0)).max(0.0);
        let diameter = diameter + config.border_width * 2.0;

        let base = Base {
            width: diameter,
            ..Base::new(config, diameter)
        };

        let indicator_count = circle.indicator_count;
        let spacing_angle = circle
            .spacing_angle
            .min(2.0 * std::f64::consts::PI / indicator_count as f64);
        Self {
            base,
            indicator_count,
            inner_radius,
            spacing_angle,
            lock_color: circle.lock_color.into(),
        }
    }

    fn blink(&mut self, cr: &cairo::Context) {
        let height = (self.height / 3.0).round();
        self.base.blink(
            cr,
            height,
            (self.width / 2.0).round(),
            (self.height / 2.0 - height / 2.0).round(),
            BlinkBg::Pattern(&self.lock_color),
        );
    }

    // TODO
    pub fn update(&mut self, cr: &cairo::Context, background: &super::Pattern) {
        if self.dirty {
            trace!("indicator dirty");
            self.clear(cr, background);
            self.paint(cr);
        } else if self.dirty_blink {
            trace!("dirty blink");
            self.blink(cr);
        }
    }

    pub fn paint(&mut self, cr: &cairo::Context) {
        assert!(self.width != 0.0);
        cr.save();

        // calculate coordinates and dimensions inside the borders:
        let x = self.x + self.border_width;
        let y = self.y + self.border_width;
        cr.translate(x, y);
        let diameter = self.width - 2.0 * self.border_width;

        let middle = (diameter / 2.0, diameter / 2.0);
        let stroke_radius = diameter / 2.0 + self.border_width / 2.0;

        // draw the lock icon
        let lock_width = diameter / 5.0;
        cr.save();
        cr.translate(
            (diameter - lock_width) / 2.0,
            (diameter - lock_width * 2.0) / 2.0,
        );
        cr.new_path();
        cr.arc(
            lock_width / 2.0,
            lock_width / 2.0,
            lock_width / 2.0,
            0.0,
            2.0 * std::f64::consts::PI,
        );
        cr.move_to(lock_width / 2.0, 0.0);
        cr.line_to(lock_width, lock_width * 2.0);
        cr.line_to(0.0, lock_width * 2.0);
        cr.close_path();
        cr.set_source(&self.lock_color);
        cr.fill();
        cr.restore();

        // draw the indicators
        cr.new_path();
        cr.arc(
            middle.0,
            middle.1,
            self.width / 2.0,
            0.0,
            2.0 * std::f64::consts::PI,
        );

        cr.new_sub_path();
        cr.arc(
            middle.0,
            middle.1,
            self.inner_radius - self.border_width / 2.0,
            0.0,
            2.0 * std::f64::consts::PI,
        );
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.clip();

        let bfg = if self.has_focus {
            &self.border_pattern_focused
        } else {
            &self.border_pattern
        };
        cr.set_line_width(self.border_width);
        for ix in 0..self.indicator_count {
            let is_lid = self.pass_len > 0
                && (self.show_selection_do
                    || (i64::from(self.pass_len) - 1) % self.indicator_count as i64 == ix as i64);

            let angle: f64 = 2.0 * std::f64::consts::PI / self.indicator_count as f64;
            let from_angle = angle * (ix as f64 - 1.0);
            let to_angle = angle * ix as f64 - self.spacing_angle;

            cr.new_path();
            cr.arc(middle.0, middle.1, stroke_radius, from_angle, to_angle);
            cr.line_to(middle.0, middle.1);
            cr.close_path();
            let pat = if is_lid {
                &self.indicator_pattern
            } else {
                &self.background
            };
            cr.set_source(pat);
            cr.fill_preserve();
            cr.set_source(bfg);
            cr.stroke();

            cr.new_path();
            cr.arc(middle.0, middle.1, self.inner_radius, from_angle, to_angle);
            cr.set_source(bfg);
            cr.stroke();
        }

        cr.restore();

        if self.has_focus && self.blink_on {
            self.blink(cr);
        }

        self.dirty = false;
    }
}

#[derive(Debug)]
pub struct IndicatorElement {
    x: f64,
    y: f64,
}

#[derive(Debug)]
pub struct Classic {
    max_count: u16,
    min_count: u16,
    // includes the border width
    element_width: f64,
    element_height: f64,
    horizontal_spacing: f64,
    radius_x: f64,
    radius_y: f64,
    indicators: Vec<IndicatorElement>,
    base: Base,
}

impl Deref for Classic {
    type Target = Base;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl DerefMut for Classic {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl Classic {
    pub fn new(
        config: config::IndicatorCommon,
        classic: config::IndicatorClassic,
        text_height: f64,
    ) -> Self {
        let border_width = config.border_width;
        let element_height = classic.element_height.unwrap_or(text_height + 2.0 * border_width);
        let height = element_height;
        let base = Base {
            height,
            blink_on: false,
            blink_do: false,
            blink_enabled: false,
            ..Base::new(config, height)
        };

        Self {
            base,
            max_count: classic.max_count,
            min_count: classic.min_count,
            element_width: classic.element_width.unwrap_or(text_height * 2.0 + 2.0 * border_width),
            element_height,
            radius_x: classic.radius_x,
            radius_y: classic.radius_y,
            horizontal_spacing: classic
                .horizontal_spacing
                .unwrap_or_else(|| (text_height / 3.0).round()),
            indicators: Vec::new(),
        }
    }

    pub fn for_width(&mut self, for_width: f64) {
        let indicator_count = min(
            max(
                ((for_width + self.horizontal_spacing)
                    / (self.element_width + self.horizontal_spacing))
                    .round() as u16,
                self.min_count,
            ),
            self.max_count,
        );
        self.width = indicator_count as f64 * (self.element_width + self.horizontal_spacing)
            - self.horizontal_spacing;

        let mut x = 0.0;
        for _ in 0..indicator_count {
            let e = IndicatorElement { x, y: 0.0 };
            self.indicators.push(e);
            x += self.element_width + self.horizontal_spacing;
        }
    }

    // TODO
    pub fn update(&mut self, cr: &cairo::Context, background: &super::Pattern) {
        if self.dirty {
            trace!("indicator dirty");
            self.clear(cr, background);
            self.paint(cr);
        }
    }

    pub fn paint(&mut self, cr: &cairo::Context) {
        trace!("paint start");
        assert!(self.width != 0.0);
        cr.save();
        cr.translate(self.x, self.y);
        cr.set_line_width(self.border_width);
        for (ix, i) in self.indicators.iter().enumerate() {
            let is_lid = self.pass_len > 0
                && (self.show_selection_do
                    || (i64::from(self.pass_len) - 1) % self.indicators.len() as i64 == ix as i64);
            super::Button::rounded_rectangle(
                cr,
                self.radius_x,
                self.radius_y,
                i.x + self.border_width / 2.0,
                i.y + self.border_width / 2.0,
                self.element_width - self.border_width,
                self.element_height - self.border_width,
            );
            let bg = if is_lid {
                &self.indicator_pattern
            } else {
                &self.background
            };
            cr.set_source(bg);
            cr.fill_preserve();
            let bp = if self.has_focus {
                &self.border_pattern_focused
            } else {
                &self.border_pattern
            };
            cr.set_source(bp);
            cr.stroke();
        }
        self.dirty = false;
        cr.restore();
        trace!("paint end");
    }
}

#[derive(Debug)]
pub struct Strings {
    base: Base,
    layout: pango::Layout,
    strings: Vec<String>,
    paste_string: String,
    paste_width: f64,
    radius_x: f64,
    radius_y: f64,
    vertical_spacing: f64,
    horizontal_spacing: f64,
    blink_spacing: f64,
    text_widths: Vec<f64>,
}

impl Deref for Strings {
    type Target = Base;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl DerefMut for Strings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl Strings {
    pub fn new(
        config: config::IndicatorCommon,
        strings_cfg: config::IndicatorStrings,
        layout: pango::Layout,
    ) -> Result<Self> {
        let mut strings = strings_cfg.strings;
        if strings.len() < 2 {
            return Err(anyhow!("must have at least 2 strings").into());
        }
        let mut sizes: Vec<(i32, i32)> = strings
            .iter()
            .map(|s| {
                layout.set_text(s);
                layout.get_pixel_size()
            })
            .collect();
        let blink_spacing = 8.0;
        let width =
            sizes.iter().map(|s| s.0).max().unwrap() as f64 + 2.0 * strings_cfg.horizontal_spacing + blink_spacing + 2.0 * config.border_width;
        let paste_width = sizes.pop().unwrap().0 as f64;
        let paste_string = strings.pop().unwrap();
        // every string with the same font should have the same logical height
        let height = sizes[0].1 as f64 + 2.0 * strings_cfg.vertical_spacing + 2.0 * config.border_width;
        let base = Base {
            width,
            ..Base::new(config, height)
        };

        Ok(Self {
            base,
            layout,
            strings,
            paste_string,
            paste_width,
            radius_x: strings_cfg.radius_x,
            radius_y: strings_cfg.radius_x,
            horizontal_spacing: strings_cfg.horizontal_spacing,
            vertical_spacing: strings_cfg.vertical_spacing,
            text_widths: sizes.into_iter().map(|(w, _)| w as f64).collect(),
            blink_spacing,
        })
    }

    pub fn paint(&mut self, cr: &cairo::Context) {
        trace!("paint start");
        assert!(self.width != 0.0);
        cr.save();
        cr.translate(self.x, self.y);
        super::Button::rounded_rectangle(
            cr,
            self.radius_x,
            self.radius_y,
            self.border_width / 2.0,
            self.border_width / 2.0,
            self.width - self.border_width,
            self.height - self.border_width,
        );
        cr.set_source(&self.background);
        cr.set_line_width(self.border_width);
        cr.fill_preserve();
        let bp = if self.has_focus {
            &self.border_pattern_focused
        } else {
            &self.border_pattern
        };
        cr.set_source(bp);
        cr.stroke();

        if self.pass_len > 0 {
            let (text, text_width) = if self.show_selection_do {
                (&self.paste_string, self.paste_width)
            } else {
                let idx = usize::try_from(self.pass_len - 1).unwrap() % self.strings.len();
                (&self.strings[idx], self.text_widths[idx])
            };
            self.layout.set_text(text);
            cr.set_source(&self.foreground);
            let mvx = if self.blink_enabled {
                self.blink_spacing + (self.width - self.blink_spacing - text_width) / 2.0
            } else {
                (self.width - text_width) / 2.0
            };
            cr.move_to(mvx, self.vertical_spacing);
            pangocairo::show_layout(&cr, &self.layout);
        }

        cr.restore();

        if self.has_focus && self.blink_on {
            self.blink(cr);
        }

        self.dirty = false;
        trace!("paint end");
    }

    // TODO
    pub fn update(&mut self, cr: &cairo::Context, background: &super::Pattern) {
        if self.dirty {
            trace!("indicator dirty");
            self.clear(cr, background);
            self.paint(cr);
        } else if self.dirty_blink {
            trace!("dirty blink");
            self.blink(cr);
        }
    }

    fn blink(&mut self, cr: &cairo::Context) {
        self.base.blink(
            cr,
            self.height - 2.0 * self.vertical_spacing - 2.0 * self.border_width,
            self.border_width + self.blink_spacing,
            self.vertical_spacing + self.border_width,
            BlinkBg::Background,
        );
    }
}
