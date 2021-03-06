use std::cmp::{max, min};
use std::convert::TryFrom as _;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::time::Duration;

use log::trace;
use rand::seq::SliceRandom as _;
use tokio::time::{sleep, Instant, Sleep};

use super::Pattern;
use crate::backbuffer::FrameId;
use crate::config;
use crate::errors::Result;
use crate::secret::{Passphrase, SecBuf};

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
    pub(super) dirty: bool,
    pub(super) dirty_blink: bool,
    blink_do: bool,
    blink_enabled: bool,
    blink_on: bool,
    show_selection_do: bool,
    blink_timeout: Pin<Box<Sleep>>,
    show_selection_timeout: Pin<Box<Sleep>>,
    pub pass: SecBuf<char>,
    debug: bool,
}

impl Base {
    pub fn new(config: config::IndicatorCommon, height: f64, debug: bool) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height,
            debug,
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
            dirty: false,
            dirty_blink: false,
            blink_on: true,
            blink_do: config.blink,
            blink_enabled: config.blink,
            show_selection_do: false,
            blink_timeout: Box::pin(sleep(Duration::from_millis(800))),
            show_selection_timeout: Box::pin(sleep(Duration::from_millis(0))),
            pass: SecBuf::new(vec!['X'; 512]),
        }
    }

    pub fn pass_delete(&mut self) -> bool {
        let dirty = self.key_pressed();
        if self.pass.len > 0 {
            self.pass.len -= 1;
            self.dirty = true;
            return true;
        }
        dirty
    }

    pub fn pass_clear(&mut self) -> bool {
        let dirty = self.key_pressed();
        if self.pass.len != 0 {
            self.pass.len = 0;
            self.dirty = true;
            return true;
        }
        dirty
    }

    pub fn pass_insert(&mut self, s: &str, pasted: bool) -> bool {
        let dirty = self.key_pressed();
        let mut inserted = false;
        for c in s.chars() {
            if self.pass.push(c).is_err() {
                break;
            }
            inserted = true;
        }
        if inserted {
            if pasted {
                self.show_selection();
            }
            self.dirty = true;
            return true;
        }
        trace!("pass insert failed");
        dirty
    }

    pub fn init_timeouts(&mut self) {
        if self.blink_do {
            self.reset_blink();
        }
    }

    // TODO
    pub fn requests_events(&mut self) -> bool {
        self.blink_do || self.show_selection_do
    }

    pub async fn handle_events(&mut self) -> bool {
        tokio::select! {
            _ = &mut self.blink_timeout, if self.blink_do => {
                self.on_blink_timeout()
            }
            _ = &mut self.show_selection_timeout, if self.show_selection_do => {
                self.on_show_selection_timeout()
            }
        }
    }

    pub fn set_painted(&mut self) {
        self.dirty = false;
        self.dirty_blink = false;
    }

    fn clear(&self, cr: &cairo::Context, background: &super::Pattern) {
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

    fn blink(
        &self,
        cr: &cairo::Context,
        height: f64,
        x: f64,
        y: f64,
        bg: Option<&Pattern>,
        sharp: bool,
    ) {
        cr.save();

        cr.translate(self.x, self.y);

        if self.has_focus && self.blink_on {
            cr.set_source(&self.foreground);
            if sharp {
                cr.move_to(x.floor() + 0.5, y.round());
            } else {
                cr.move_to(x, y);
            };
            cr.rel_line_to(0.0, height);
            cr.set_line_width(1.0);
            cr.stroke();
        } else {
            cr.rectangle(x - 1.0, y - 1.0, 3.0, height + 2.0);
            cr.set_source(bg.unwrap_or(&self.background));
            cr.fill();
        };

        cr.restore();
    }

    pub fn on_show_selection_timeout(&mut self) -> bool {
        assert!(self.show_selection_do);
        self.show_selection_do = false;
        self.dirty = true;
        true
    }

    fn show_selection(&mut self) -> bool {
        self.show_selection_do = true;
        self.show_selection_timeout.as_mut().reset(
            Instant::now()
                .checked_add(Duration::from_millis(200))
                .unwrap(),
        );
        self.dirty = true;
        true
    }

    pub fn key_pressed(&mut self) -> bool {
        let mut ret = false;
        if self.blink_enabled {
            if !self.blink_on {
                self.dirty_blink = true;
                ret = true;
            }
            self.blink_on = true;
            self.reset_blink();
        }
        ret
    }

    pub fn set_focused(&mut self, is_focused: bool) -> bool {
        self.dirty = self.dirty || is_focused != self.has_focus;
        self.has_focus = is_focused;
        if self.blink_enabled {
            self.blink_on = is_focused;
            if is_focused {
                self.reset_blink();
            }
            self.blink_do = is_focused;
        }
        self.dirty
    }

    pub fn on_blink_timeout(&mut self) -> bool {
        trace!("blink timeout");
        self.blink_on = !self.blink_on;
        self.dirty_blink = true;
        self.reset_blink();
        true
    }

    fn reset_blink(&mut self) {
        let duration = if self.blink_on {
            Duration::from_millis(800)
        } else {
            Duration::from_millis(400)
        };
        self.blink_timeout
            .as_mut()
            .reset(Instant::now().checked_add(duration).unwrap());
    }
}

#[derive(Debug)]
pub struct Circle {
    base: Base,
    indicator_count: u32,
    inner_radius: f64,
    spacing_angle: f64,
    light_up: bool,
    rotate: bool,
    frame: u64,
    frame_increment: f64,
    frame_increment_start: f64,
    frame_increment_gain: f64,
    angle: f64,
    animation_distance: f64,
    rotation: f64,
    lock_color: Pattern,
    last_animation_serial: Option<FrameId>,
    oldlen: usize,
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
        debug: bool,
    ) -> Self {
        let diameter = circle.diameter.unwrap_or(text_height * 3.0);
        let inner_radius =
            (diameter / 2.0 - circle.indicator_width.unwrap_or(diameter / 4.0)).max(0.0);
        let diameter = diameter + config.border_width * 2.0;

        let base = Base {
            width: diameter,
            blink_on: config.blink,
            ..Base::new(config, diameter, debug)
        };

        let indicator_count = circle.indicator_count;
        let spacing_angle = circle
            .spacing_angle
            .min(2.0 * std::f64::consts::PI / indicator_count as f64);
        let frame_increment_start = circle.rotation_speed_start;
        Self {
            base,
            indicator_count,
            inner_radius,
            spacing_angle,
            light_up: circle.light_up,
            rotate: circle.rotate,
            lock_color: circle.lock_color.into(),
            frame: 0,
            frame_increment: frame_increment_start,
            frame_increment_start,
            frame_increment_gain: circle.rotation_speed_gain,
            last_animation_serial: None,
            angle: 2.0 * std::f64::consts::PI / indicator_count as f64,
            animation_distance: 0.0,
            rotation: 0.0,
            oldlen: 0,
        }
    }

    pub fn pass_delete(&mut self) -> bool {
        let ret = self.base.pass_delete();
        if self.rotate {
            self.init_rotation();
        }
        ret
    }

    pub fn pass_clear(&mut self) -> bool {
        let ret = self.base.pass_clear();
        if self.rotate {
            self.init_rotation();
        }
        ret
    }

    pub fn pass_insert(&mut self, s: &str, pasted: bool) -> bool {
        let ret = self.base.pass_insert(s, pasted);
        if self.rotate {
            self.init_rotation();
        }
        ret
    }

    pub fn into_pass(self) -> Passphrase {
        Passphrase(self.base.pass)
    }

    fn init_rotation(&mut self) {
        trace!("run animation");
        let full_round = 2.0 * std::f64::consts::PI;
        self.rotation %= full_round;
        self.animation_distance += (self.pass.len as i64 - self.oldlen as i64) as f64
            * (self.angle / self.indicator_count as f64);
        self.oldlen = self.pass.len;
        if self.animation_distance > 2.0 * full_round {
            self.animation_distance = self.animation_distance % full_round + full_round;
        } else if self.animation_distance < 2.0 * -full_round {
            self.animation_distance = self.animation_distance % full_round - full_round;
        }
    }

    pub fn on_displayed(&mut self, serial: FrameId) -> bool {
        trace!(
            "serial {:?}, last_animation_serial {:?}",
            serial,
            self.last_animation_serial
        );
        if let Some(s) = self.last_animation_serial {
            assert!(self.animation_distance != 0.0);
            if serial == s {
                self.last_animation_serial = None;
                let mut animation_running = true;
                if self.animation_distance > 0.0 {
                    self.rotation += self.frame_increment.min(self.animation_distance);
                    self.animation_distance -= self.frame_increment;
                    if self.animation_distance <= 0.0 {
                        animation_running = false;
                    }
                    trace!(
                        "animation_distance {}, rotation {}",
                        self.animation_distance,
                        self.rotation
                    );
                } else {
                    self.rotation -= self.frame_increment.min(-self.animation_distance);
                    self.animation_distance += self.frame_increment;
                    if self.animation_distance >= 0.0 {
                        animation_running = false;
                    }
                }

                if animation_running {
                    self.frame_increment *= self.frame_increment_gain;
                } else {
                    self.frame_increment = self.frame_increment_start;
                    self.animation_distance = 0.0;
                }

                self.dirty = true;
                return true;
            } else {
                trace!("last animated frame might not have been shown yet");
            }
        }
        false
    }

    fn blink(&self, cr: &cairo::Context) {
        let height = (self.height / 3.0).round();
        self.base.blink(
            cr,
            height,
            self.width / 2.0,
            (self.height - height) / 2.0,
            Some(&self.lock_color),
            false,
        );
    }

    pub fn set_painted(&mut self, serial: FrameId) {
        trace!(
            "set_painted serial {:?}, animation_distance {}",
            serial,
            self.animation_distance
        );
        if self.animation_distance != 0.0 && self.last_animation_serial.is_none() {
            self.last_animation_serial = Some(serial);
        }
        self.base.set_painted()
    }

    // TODO
    pub fn update(&self, cr: &cairo::Context, background: &super::Pattern) {
        if self.dirty {
            trace!("indicator dirty");
            self.clear(cr, background);
            self.paint(cr);
        } else if self.dirty_blink {
            trace!("dirty blink");
            self.blink(cr);
        }
    }

    pub fn paint(&self, cr: &cairo::Context) {
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

        cr.set_line_width(self.border_width);
        for ix in 0..self.indicator_count {
            let is_lid = self.light_up
                && self.pass.len > 0
                && (self.show_selection_do
                    || (i64::try_from(self.pass.len).unwrap() - 1) % self.indicator_count as i64
                        == if self.rotate {
                            self.indicator_count - 1 - ix
                        } else {
                            ix
                        } as i64);

            let rotation = self.rotation % (2.0 * std::f64::consts::PI);
            let from_angle = self.angle * (ix as f64 - 1.0) + rotation;
            let to_angle = self.angle * ix as f64 - self.spacing_angle + rotation;

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
            let bfg = if self.has_focus {
                &self.border_pattern_focused
            } else {
                &self.border_pattern
            };
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
        debug: bool,
    ) -> Self {
        let border_width = config.border_width;
        let element_height = classic
            .element_height
            .unwrap_or(text_height + 2.0 * border_width);
        let height = element_height;
        let base = Base {
            height,
            blink_on: false,
            blink_do: false,
            blink_enabled: false,
            ..Base::new(config, height, debug)
        };

        Self {
            base,
            max_count: classic.max_count,
            min_count: classic.min_count,
            element_width: classic
                .element_width
                .unwrap_or(text_height * 2.0 + 2.0 * border_width),
            element_height,
            radius_x: classic.radius_x,
            radius_y: classic.radius_y,
            horizontal_spacing: classic
                .horizontal_spacing
                .unwrap_or_else(|| (text_height / 3.0).round()),
            indicators: Vec::new(),
        }
    }

    pub fn into_pass(self) -> Passphrase {
        Passphrase(self.base.pass)
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
    pub fn update(&self, cr: &cairo::Context, background: &super::Pattern) {
        if self.dirty {
            trace!("indicator dirty");
            self.clear(cr, background);
            self.paint(cr);
        }
    }

    pub fn paint(&self, cr: &cairo::Context) {
        trace!("paint start");
        assert!(self.width != 0.0);
        cr.save();
        cr.translate(self.x, self.y);
        cr.set_line_width(self.border_width);
        for (ix, i) in self.indicators.iter().enumerate() {
            let is_lid = self.pass.len > 0
                && (self.show_selection_do
                    || (i64::try_from(self.pass.len).unwrap() - 1) % self.indicators.len() as i64
                        == ix as i64);
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
        cr.restore();
        trace!("paint end");
    }
}

#[derive(Debug)]
enum StringType {
    Disco(Disco),
    Custom(Custom),
    Asterisk(Asterisk),
}

impl StringType {
    pub fn use_cursor(&self) -> bool {
        match self {
            Self::Disco(..) => false,
            Self::Custom(..) => false,
            Self::Asterisk(..) => true,
        }
    }

    pub fn for_width(&mut self, layout: &pango::Layout, for_width: f64) -> i32 {
        match self {
            Self::Disco(disco) => disco.for_width(layout, for_width),
            Self::Custom(custom) => custom.width,
            Self::Asterisk(asterisk) => asterisk.for_width(layout, for_width),
        }
    }

    pub fn set_text(&mut self, layout: &pango::Layout, pass: &SecBuf<char>, show_paste: bool) {
        match self {
            Self::Disco(disco) => disco.set_text(layout, pass, show_paste),
            Self::Custom(custom) => custom.set_text(layout, pass, show_paste),
            Self::Asterisk(asterisk) => asterisk.set_text(layout, pass),
        }
    }

    pub fn height(&self) -> i32 {
        match self {
            Self::Disco(disco) => disco.height,
            Self::Custom(custom) => custom.height,
            Self::Asterisk(asterisk) => asterisk.height,
        }
    }
}

#[derive(Debug)]
pub struct Strings {
    base: Base,
    strings: StringType,
    //paste_string: String,
    //paste_width: f64,
    radius_x: f64,
    radius_y: f64,
    vertical_spacing: f64,
    horizontal_spacing: f64,
    //text_widths: Vec<f64>,
    index: (i32, i32),
    blink_spacing: f64,
    layout: pango::Layout,
    show_plain: bool,
    cursor: usize,
    hover: bool,
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
        debug: bool,
    ) -> Result<Self> {
        let strings = match strings_cfg.strings {
            config::StringType::Asterisk { asterisk } => {
                StringType::Asterisk(Asterisk::new(asterisk, &layout))
            }
            config::StringType::Disco { disco } => StringType::Disco(Disco::new(disco, &layout)),
            config::StringType::Custom { custom } => {
                StringType::Custom(Custom::new(custom, &layout))
            }
        };
        let height = strings.height() as f64
            + 2.0 * strings_cfg.vertical_spacing
            + 2.0 * config.border_width;
        let base = Base {
            ..Base::new(config, height, debug)
        };

        layout.set_height(strings.height() * pango::SCALE);
        layout.set_single_paragraph_mode(true);

        let blink_spacing = if strings.use_cursor() { 0.0 } else { 8.0 };
        Ok(Self {
            base,
            strings,
            radius_x: strings_cfg.radius_x,
            radius_y: strings_cfg.radius_x,
            horizontal_spacing: strings_cfg.horizontal_spacing,
            vertical_spacing: strings_cfg.vertical_spacing,
            blink_spacing,
            index: (0, 0),
            layout,
            show_plain: false,
            cursor: 0,
            hover: false,
        })
    }

    pub fn is_inside(&mut self, x: f64, y: f64) -> bool {
        x >= self.x + self.border_width
            && x < self.x + self.width - self.border_width
            && y >= self.y + self.border_width
            && y < self.y + self.height - self.border_width
    }

    pub fn set_hover(&mut self, hover: bool, xcontext: &crate::event::XContext) -> Result<()> {
        if self.strings.use_cursor() || self.show_plain {
            if hover && !self.hover {
                xcontext.set_input_cursor()?;
            } else if !hover && self.hover {
                xcontext.set_default_cursor()?;
            }
            self.hover = hover;
        }
        Ok(())
    }

    pub fn pass_clear(&mut self) -> bool {
        let dirty = self.key_pressed();
        if self.pass.len != 0 {
            self.cursor = 0;
            self.pass.len = 0;
            self.set_text();
            self.dirty = true;
            return true;
        }
        dirty
    }

    pub fn pass_insert(&mut self, s: &str, pasted: bool) -> bool {
        trace!("pass insert {}", self.cursor);
        let ret = self.base.key_pressed();
        let cursor = self.cursor;
        let inserted = self.pass.insert_many(cursor, s.chars(), s.chars().count());
        if inserted > 0 {
            if pasted {
                self.show_selection();
            }
            self.set_text();
            self.cursor += inserted;
            self.dirty = true;
            trace!("pass inserted");
            return true;
        }
        ret
    }

    pub fn pass_delete(&mut self) -> bool {
        trace!("pass delete {}", self.cursor);
        let ret = self.base.key_pressed();
        if self.cursor == 0 {
            return ret;
        }
        let i = self.cursor - 1;
        self.pass.delete(i);
        self.set_text();
        self.cursor -= 1;
        self.dirty = true;
        true
    }

    pub fn move_cursor(&mut self, direction: i32) -> bool {
        if !self.strings.use_cursor() && !self.show_plain {
            return false;
        }
        trace!("move cursor {}", self.cursor);
        let dirty = self.key_pressed();
        let new_cursor = self
            .layout
            .move_cursor_visually(true, self.cursor_bytes(), 0, direction);
        if new_cursor.0 != std::i32::MAX && new_cursor.0 != -1 {
            self.cursor = self.cursor_chars(new_cursor.0, new_cursor.1);

            self.dirty = true;
            return true;
        }
        dirty
    }

    fn cursor_chars(&self, idx: i32, trailing: i32) -> usize {
        assert!(self.strings.use_cursor() || self.show_plain);
        let gs = self.layout.get_text().unwrap();
        let s = gs.as_str();
        let cb = usize::try_from(idx).unwrap();
        let f = s
            .char_indices()
            .enumerate()
            .find(|(_, (b, _))| *b == cb)
            .unwrap();
        f.0 + usize::try_from(trailing).unwrap()
    }

    fn cursor_bytes(&self) -> i32 {
        assert!(self.strings.use_cursor() || self.show_plain);
        if self.cursor == 0 {
            return 0;
        }
        let gs = self.layout.get_text().unwrap();
        let s = gs.as_str();
        let indice = s.char_indices().nth(self.cursor - 1).unwrap();
        i32::try_from(indice.0 + indice.1.len_utf8()).unwrap()
    }

    pub fn for_width(&mut self, for_width: f64) {
        self.width = self.strings.for_width(&self.layout, for_width) as f64
            + 2.0 * self.horizontal_spacing
            + self.blink_spacing
            + 2.0 * self.border_width;
    }

    pub fn toggle_plaintext(&mut self) {
        self.show_plain = !self.show_plain;
        if self.show_plain {
            self.layout.set_ellipsize(pango::EllipsizeMode::Middle);
        }

        self.set_text();
    }

    pub fn into_pass(self) -> Passphrase {
        Passphrase(self.base.pass)
    }

    pub fn paint(&self, cr: &cairo::Context) {
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

        cr.save();
        cr.translate(
            self.blink_spacing + self.horizontal_spacing + self.border_width,
            self.vertical_spacing + self.border_width,
        );
        cr.set_source(&self.foreground);
        cr.move_to(0.0, 0.0);
        pangocairo::show_layout(&cr, &self.layout);
        // TODO text is drawn too high
        // pangocairo::show_layout_line(&cr, &self.layout.get_line_readonly(self.layout.get_line_count() - 1).unwrap());
        cr.restore();

        cr.restore();

        if self.has_focus && self.blink_on {
            self.blink(cr);
        }

        trace!("paint end");
    }

    pub fn on_show_selection_timeout(&mut self) -> bool {
        self.base.on_show_selection_timeout();
        self.set_text();
        true
    }

    pub async fn handle_events(&mut self) -> bool {
        tokio::select! {
            _ = &mut self.base.blink_timeout, if self.base.blink_do => {
                self.on_blink_timeout()
            }
            _ = &mut self.base.show_selection_timeout, if self.base.show_selection_do => {
                self.on_show_selection_timeout()
            }
        }
    }

    // TODO
    pub fn update(&self, cr: &cairo::Context, background: &super::Pattern) {
        if self.dirty {
            trace!("indicator dirty");
            self.clear(cr, background);
            self.paint(cr);
        } else if self.dirty_blink {
            trace!("dirty blink");
            self.blink(cr);
        }
    }

    // return (inside, dirty)
    pub fn set_cursor(&mut self, x: f64, y: f64) -> (bool, bool) {
        if !self.show_plain && !self.strings.use_cursor() {
            return (false, false);
        }

        if self.is_inside(x, y) {
            let rec = self.layout.get_extents().1;
            let (inside, idx, trailing) = self.layout.xy_to_index(
                min(
                    max(
                        ((x - self.x
                            - self.blink_spacing
                            - self.horizontal_spacing
                            - self.border_width)
                            * pango::SCALE as f64) as i32,
                        rec.x,
                    ),
                    rec.x + rec.width - 1,
                ),
                min(
                    max(
                        ((y - self.y - self.vertical_spacing - self.border_width)
                            * pango::SCALE as f64) as i32,
                        rec.y,
                    ),
                    rec.y + rec.height - 1,
                ),
            );
            if inside {
                self.key_pressed();
                self.cursor = self.cursor_chars(idx, trailing);
                self.dirty = true;
                return (true, true);
            } else {
                assert!(
                    self.pass.len == 0,
                    "click x:{}, y: {}, {} {} {}",
                    x,
                    y,
                    inside,
                    idx,
                    trailing
                );
                return (false, false);
            }
        }
        (false, false)
    }

    fn set_text(&mut self) {
        if self.show_plain {
            let mut buf: SecBuf<u8> = SecBuf::new(vec![0; 4 * self.pass.len]);
            for c in self.pass.unsecure() {
                let ret = c.encode_utf8(&mut buf.buf.unsecure_mut()[buf.len..]);
                buf.len += ret.len();
            }
            let s = unsafe { std::str::from_utf8_unchecked(buf.unsecure()) };
            // well this isn't stored in any secure way anyway
            self.layout.set_text(s);
        } else {
            self.strings
                .set_text(&self.layout, &self.base.pass, self.show_selection_do);
        }
        self.dirty = true;
    }

    fn blink(&self, cr: &cairo::Context) {
        if self.has_focus && self.blink_on {
            let pos = if self.show_plain || self.strings.use_cursor() {
                (self.layout.get_cursor_pos(self.cursor_bytes()).0.x as f64 / pango::SCALE as f64)
                    .round()
                    + self.blink_spacing
            } else {
                0.0
            };
            self.base.blink(
                cr,
                self.height - 2.0 * self.vertical_spacing - 2.0 * self.border_width,
                self.border_width + self.horizontal_spacing + pos,
                self.vertical_spacing + self.border_width,
                None,
                true,
            )
        } else {
            self.paint(cr)
        }
    }
}

#[derive(Debug)]
struct Custom {
    height: i32,
    width: i32,
    strings: Vec<String>,
}

impl Custom {
    pub fn new(config: config::Custom, layout: &pango::Layout) -> Self {
        let sizes: Vec<(i32, i32)> = config
            .strings
            .iter()
            .map(|s| {
                layout.set_text(s);
                layout.get_pixel_size()
            })
            .collect();
        // every string with the same font should have the same logical height
        let height = sizes[0].1;
        let width = sizes.into_iter().map(|(w, _)| w).max().unwrap();
        layout.set_width(width * pango::SCALE);
        layout.set_alignment(config.alignment.into());
        layout.set_justify(config.justify);
        let mut strings = config.strings;
        if config.randomize {
            let mut rand = rand::thread_rng();
            strings[1..].shuffle(&mut rand);
        }
        Self {
            height,
            width,
            strings,
        }
    }

    pub fn set_text(&mut self, layout: &pango::Layout, pass: &SecBuf<char>, show_paste: bool) {
        if pass.len == 0 {
            layout.set_text("");
            return;
        }
        let idx = if show_paste {
            0
        } else {
            (pass.len as usize - 1) % (self.strings.len() - 1) + 1
        };

        layout.set_text(&self.strings[idx]);
    }
}

#[derive(Debug)]
struct Disco {
    height: i32,
    widths: Vec<f64>,
    dancer_max_width: f64,
    separator_width: f64,
    dancer_count: u16,
    config: config::Disco,
}

impl Disco {
    pub const DANCER: &'static [&'static str] = &["┗(･o･)┛", "┏(･o･)┛", "┗(･o･)┓", "┏(･o･)┓"];
    pub const SEPARATOR: &'static str = " ♪ ";

    pub fn new(config: config::Disco, layout: &pango::Layout) -> Self {
        trace!("disco new start");
        let strings = Self::DANCER;
        let sizes: Vec<(i32, i32)> = strings
            .iter()
            .map(|s| {
                layout.set_text(s);
                layout.get_pixel_size()
            })
            .collect();
        // every string with the same font should have the same logical height
        let height = sizes[0].1;
        let widths = sizes.iter().map(|(w, _)| *w as f64).collect();
        let dancer_max_width = sizes.into_iter().map(|(w, _)| w).max().unwrap() as f64;
        layout.set_text("");
        trace!("disco new end");
        Self {
            height,
            widths,
            dancer_max_width,
            separator_width: layout.get_pixel_size().1 as f64,
            config,
            dancer_count: 0,
        }
    }

    pub fn for_width(&mut self, layout: &pango::Layout, for_width: f64) -> i32 {
        trace!("for_width start");
        self.dancer_count = min(
            max(
                ((for_width + self.separator_width)
                    / (self.dancer_max_width + self.separator_width))
                    .round() as u16,
                self.config.min_count,
            ),
            self.config.max_count,
        );
        let last = if self.config.three_states { 4 } else { 3 };
        let width = (0..last)
            .map(|l| {
                self.set_text_do(layout, l, l == 0);
                layout.get_pixel_size().0
            })
            .max()
            .unwrap();
        // would not match the above:
        //let width = self.dancer_count as f64 * (self.dancer_max_width + self.separator_width)
        //- self.separator_width;
        trace!("for_width end");
        layout.set_width(width * pango::SCALE);
        layout.set_text("");
        width
    }

    pub fn set_text(&mut self, layout: &pango::Layout, pass: &SecBuf<char>, show_paste: bool) {
        self.set_text_do(layout, pass.len, show_paste)
    }

    fn set_text_do(&mut self, layout: &pango::Layout, pass_len: usize, show_paste: bool) {
        if pass_len == 0 {
            layout.set_text("");
            return;
        }
        let mut buf = String::with_capacity(
            (Self::DANCER[0].len() + Self::SEPARATOR.len()) * usize::from(self.dancer_count),
        );
        for i in 0..self.dancer_count {
            let states = if self.config.three_states { 3 } else { 2 };
            let idx: usize = if show_paste {
                0
            } else {
                (pass_len % states) as u8 + 1
            }
            .into();
            buf.push_str(Self::DANCER[idx]);
            if i + 1 != self.dancer_count {
                buf.push_str(Self::SEPARATOR);
            }
        }
        layout.set_text(&buf);
    }
}

#[derive(Debug)]
struct Asterisk {
    height: i32,
    asterisk_width: f64,
    asterisk: String,
    count: u16,
    min_count: u16,
    max_count: u16,
}

impl Asterisk {
    pub fn new(config: config::Asterisk, layout: &pango::Layout) -> Self {
        let asterisk: String = config.asterisk;
        layout.set_text(&asterisk);
        let (asterisk_width, height) = layout.get_pixel_size();
        layout.set_alignment(config.alignment.into());
        layout.set_text("");
        Self {
            height,
            asterisk_width: asterisk_width as f64,
            asterisk,
            min_count: config.min_count,
            max_count: config.max_count,
            count: 0,
        }
    }

    pub fn for_width(&mut self, layout: &pango::Layout, for_width: f64) -> i32 {
        self.count = min(
            max(
                (for_width / self.asterisk_width).round() as u16,
                self.min_count,
            ),
            self.max_count,
        );
        layout.set_text(&self.asterisk.repeat(self.count.into()));
        let w = layout.get_pixel_size().0;
        layout.set_width(w * pango::SCALE);
        layout.set_text("");
        w
    }

    pub fn set_text(&mut self, layout: &pango::Layout, pass: &SecBuf<char>) {
        layout.set_ellipsize(pango::EllipsizeMode::Start);

        if pass.len == 0 {
            layout.set_text("");
            return;
        }

        layout.set_text(&self.asterisk.repeat(pass.len));
    }
}
