use serde::{Deserialize, Serialize};
use std::cmp::{max, min};
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use log::trace;
use tokio::time::{sleep, Instant, Sleep};

use super::Pattern;
use crate::config;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Type {
    Circle,
    Classic,
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
    lock_color: Pattern,
    border_pattern: Pattern,
    border_pattern_focused: Pattern,
    indicator_color: Pattern,
    pass_len: u32,
    pub(super) dirty: bool,
    pub(super) dirty_blink: bool,
    pub blink_do: bool,
    blink_enabled: bool,
    blink_on: bool,
    pub show_selection_do: bool,
}

impl Base {
    pub fn on_show_selection_timeout(&mut self) -> bool {
        self.show_selection_do = false;
        self.dirty = true;
        true
    }
    pub fn show_selection(&mut self, pass_len: usize, show_selection_timeout: &mut Sleep) -> bool {
        self.pass_len = pass_len as u32;
        self.show_selection_do = true;
        show_selection_timeout.reset(
            Instant::now()
                .checked_add(Duration::from_millis(200))
                .unwrap(),
        );
        self.dirty = true;
        true
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
    text_height: f64,
    base: Base,
    indicator_count: f64,
    inner_radius: f64,
    spacing: f64,
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
    pub fn new(config: config::Indicator, text_height: f64) -> Self {
        let border_width = config.border_width;

        let diameter = config.type_circle.diameter.unwrap_or(text_height * 3.0);
        let spacing = config.type_circle.spacing;
        let inner_radius = (diameter / 2.0
            - spacing
            - config
                .type_circle
                .indicator_width
                .unwrap_or(diameter / 4.0 - spacing))
        .max(0.0);
        let diameter = diameter + border_width * 2.0;
        let blink_enabled = config.blink;

        let base = Base {
            x: 0.0,
            y: 0.0,
            width: diameter,
            height: diameter,
            border_width,
            lock_color: config.lock_color.into(),
            foreground: config.foreground.into(),
            background: Pattern::get_pattern(
                diameter - border_width,
                config.background,
                config.background_stop,
            ),
            border_pattern: config.border_color.into(),
            border_pattern_focused: config.border_color_focused.into(),
            indicator_color: Pattern::get_pattern(
                diameter - border_width,
                config.indicator_color,
                config.indicator_color_stop,
            ),
            has_focus: false,
            pass_len: 0,
            dirty: false,
            dirty_blink: false,
            blink_on: blink_enabled,
            blink_do: blink_enabled,
            blink_enabled,
            show_selection_do: false,
        };

        Self {
            base,
            text_height,
            indicator_count: config.type_circle.indicator_count,
            inner_radius,
            spacing,
        }
    }

    pub fn blink(&mut self, cr: &cairo::Context) {
        cr.save();

        cr.translate(self.x, self.y);

        if self.has_focus && self.blink_on {
            cr.set_source(&self.foreground);
            cr.move_to(self.width / 2.0, self.height / 2.0 - self.text_height / 2.0);
            cr.rel_line_to(0.0, self.text_height);
            cr.set_line_width(1.0);
            cr.stroke();
        } else {
            cr.rectangle(
                self.width / 2.0 - 1.0,
                self.height / 2.0 - self.text_height / 2.0 - 1.0,
                2.0,
                self.text_height + 2.0,
            );
            cr.set_source(&self.lock_color);
            cr.fill();
        };

        cr.restore();

        self.dirty_blink = false;
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

        cr.new_path();
        cr.arc(
            middle.0,
            middle.1,
            // adding half a border width ensures the fill touches the border:
            stroke_radius,
            0.0,
            2.0 * std::f64::consts::PI,
        );
        cr.set_source(&self.background);
        cr.fill();

        let lock_width = diameter / 4.0;
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
        //width.min(height) / 4.0 + width.min(height) / 6.0 + width.min(height) / 6.0
        cr.move_to(lock_width / 2.0, 0.0);
        cr.line_to(lock_width, lock_width * 2.0);
        cr.line_to(0.0, lock_width * 2.0);
        cr.close_path();
        cr.set_source(&self.lock_color);
        cr.fill();
        cr.restore();

        if self.pass_len > 0 {
            cr.save();
            let radius = if self.spacing > 0.0 {
                diameter / 2.0 - self.spacing
            } else {
                // ensure the indicator touches the border
                stroke_radius
            };

            cr.new_path();
            cr.arc(middle.0, middle.1, radius, 0.0, 2.0 * std::f64::consts::PI);

            cr.new_sub_path();
            cr.arc(
                middle.0,
                middle.1,
                self.inner_radius,
                0.0,
                2.0 * std::f64::consts::PI,
            );
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            cr.clip();

            let (from_angle, to_angle) = if self.show_selection_do {
                (0.0, 2.0 * std::f64::consts::PI)
            } else {
                let angle: f64 = 2.0 * std::f64::consts::PI / self.indicator_count;
                (
                    angle * (self.pass_len as f64 % self.indicator_count - 2.0),
                    angle * (self.pass_len as f64 % self.indicator_count - 1.0),
                )
            };

            cr.new_path();
            cr.arc(middle.0, middle.1, radius, from_angle, to_angle);
            cr.line_to(middle.0, middle.1);
            cr.close_path();
            cr.set_source(&self.indicator_color);
            cr.fill();

            cr.restore();
        }

        cr.new_path();
        cr.arc(
            middle.0,
            middle.1,
            stroke_radius,
            0.0,
            2.0 * std::f64::consts::PI,
        );
        let bfg = if self.has_focus {
            &self.border_pattern_focused
        } else {
            &self.border_pattern
        };
        cr.set_source(bfg);
        cr.set_line_width(self.border_width);
        cr.stroke();

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
    indicator_count: u16,
    horizontal_spacing: f64,
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
    pub fn new(config: config::Indicator, text_height: f64) -> Self {
        let element_height = config.type_classic.element_height.unwrap_or(text_height);
        let height = element_height;
        let border_width = config.border_width;
        let base = Base {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height,
            border_width,
            lock_color: config.lock_color.into(),
            foreground: config.foreground.into(),
            background: Pattern::get_pattern(
                height - border_width,
                config.background,
                config.background_stop,
            ),
            border_pattern: config.border_color.into(),
            border_pattern_focused: config.border_color_focused.into(),
            indicator_color: Pattern::get_pattern(
                height - border_width,
                config.indicator_color,
                config.indicator_color_stop,
            ),
            has_focus: false,
            pass_len: 0,
            dirty: false,
            dirty_blink: false,
            blink_on: false,
            blink_do: false,
            blink_enabled: false, // TODO implement
            show_selection_do: false,
        };

        Self {
            base,
            max_count: config.type_classic.max_count,
            min_count: config.type_classic.min_count,
            element_width: config
                .type_classic
                .element_width
                .unwrap_or_else(|| text_height * 2.0),
            element_height,
            horizontal_spacing: config.type_classic.horizontal_spacing,
            indicator_count: 0, // TODO just use vec?
            indicators: Vec::new(),
        }
    }

    pub fn for_width(&mut self, for_width: f64) {
        self.indicator_count = min(
            max(
                ((for_width + self.horizontal_spacing)
                    / (self.element_width + self.horizontal_spacing))
                    .round() as u16,
                self.min_count,
            ),
            self.max_count,
        );
        self.width = self.indicator_count as f64 * (self.element_width + self.horizontal_spacing)
            - self.horizontal_spacing;
    }

    pub fn coordinates_changed(&mut self) {
        let mut x = self.x;
        for _ in 0..self.indicator_count {
            let e = IndicatorElement { x, y: self.y };
            self.indicators.push(e);
            x += self.element_width + self.horizontal_spacing;
        }
    }

    pub fn paint(&mut self, cr: &cairo::Context) {
        assert!(self.width != 0.0);
        cr.save();
        for (ix, i) in self.indicators.iter().enumerate() {
            let is_lid = self.pass_len > 0
                && (self.show_selection_do
                    || (i64::from(self.pass_len) - 1) % self.indicators.len() as i64 == ix as i64);
            cr.rectangle(
                i.x + self.border_width / 2.0,
                i.y + self.border_width / 2.0,
                self.element_width - self.border_width,
                self.element_height - self.border_width,
            );
            let bg = if is_lid {
                &self.indicator_color
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
            cr.set_line_width(self.border_width);
            cr.stroke();
        }
        self.dirty = false;
        cr.restore();
    }
}
