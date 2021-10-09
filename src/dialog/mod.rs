use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::ffi::CStr;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::time::Duration;

use fontconfig_sys::fontconfig;
use libc::LC_ALL;
use log::{debug, info, log_enabled, trace, warn};
use pango::prelude::FontExt as _;
use tokio::time::{sleep, Instant, Sleep};
use x11rb::protocol::xproto;
use zeroize::Zeroize;

use crate::bail;
use crate::config;
use crate::config::{IndicatorType, Rgba};
use crate::errors::Result;
use crate::event::XContext;
use crate::keyboard::{
    self, keysyms, xkb_compose_feed_result, xkb_compose_status, Keyboard, Keycode,
};
use crate::secret::Passphrase;
use crate::secret::SecBuf;

pub mod indicator;
pub mod layout;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Nothing,
    Ok,
    Cancel,
    PastePrimary,
    PasteClipboard,
    PlainText,
}

pub struct Components {
    clipboard_config: Option<config::ClipboardButton>,
    plaintext_config: Option<config::TextButton>,
    labels: Vec<Label>,
    indicator_label_text: String,
    indicator_label_foreground: Option<Rgba>,
    pango_context: pango::Context,
    buttons: Vec<Button>,
    text_height: f64,
}

impl Components {
    const ACTIONS: [Action; 4] = [
        Action::Ok,
        Action::Cancel,
        Action::PasteClipboard,
        Action::PlainText,
    ];

    fn label(&mut self) -> &mut Label {
        &mut self.labels[0]
    }

    fn ok(&mut self) -> &mut Button {
        &mut self.buttons[0]
    }

    fn cancel(&mut self) -> &mut Button {
        &mut self.buttons[1]
    }

    fn clipboard(&mut self) -> &mut Button {
        if self.buttons.get_mut(2).is_none() {
            debug!("creating clipboard button");
            let config = self.clipboard_config.take().unwrap();
            let clipboard_label = Label::ClipboardLabel(ClipboardLabel::new(
                config.foreground.into(),
                self.text_height,
            ));
            self.buttons.push(Button::new(
                config.button,
                clipboard_label,
                self.text_height,
            ));
        }
        &mut self.buttons[2]
    }

    fn plaintext(&mut self) -> &mut Button {
        if self.buttons.get_mut(3).is_none() {
            debug!("creating plaintext button");
            let config = self.plaintext_config.take().unwrap();
            let layout = pango::Layout::new(&self.pango_context);
            layout.set_text(&config.label);
            let label = Label::TextLabel(TextLabel::new(config.foreground.into(), layout));
            self.buttons
                .push(Button::new(config.button, label, self.text_height));
        }
        &mut self.buttons[3]
    }

    fn indicator_label(&mut self) -> &mut Label {
        if self.labels.get_mut(1).is_none() {
            debug!("creating indicator label");
            let indicator_layout = pango::Layout::new(&self.pango_context);
            indicator_layout.set_text(&self.indicator_label_text);
            let indicator_label = Label::TextLabel(TextLabel::new(
                self.indicator_label_foreground.take().unwrap().into(),
                indicator_layout,
            ));

            self.labels.push(indicator_label);
        }
        &mut self.labels[1]
    }
}

// https://users.rust-lang.org/t/performance-implications-of-box-trait-vs-enum-delegation/11957
#[derive(Debug)]
pub enum Pattern {
    Solid(cairo::SolidPattern),
    Linear(cairo::LinearGradient),
}

impl Pattern {
    pub fn get_pattern(fill_height: f64, start: Rgba, end: Option<Rgba>) -> Self {
        if let Some(end) = end {
            let grad = cairo::LinearGradient::new(0.0, 0.0, 0.0, fill_height);
            grad.add_color_stop_rgba(
                0.0,
                f64::from(start.red) / f64::from(u8::MAX),
                f64::from(start.green) / f64::from(u8::MAX),
                f64::from(start.blue) / f64::from(u8::MAX),
                f64::from(start.alpha) / f64::from(u8::MAX),
            );
            grad.add_color_stop_rgba(
                1.0,
                f64::from(end.red) / f64::from(u8::MAX),
                f64::from(end.green) / f64::from(u8::MAX),
                f64::from(end.blue) / f64::from(u8::MAX),
                f64::from(end.alpha) / f64::from(u8::MAX),
            );
            Self::Linear(grad)
        } else {
            Self::from(start)
        }
    }
}

impl From<Rgba> for Pattern {
    fn from(val: Rgba) -> Self {
        Self::Solid(cairo::SolidPattern::from_rgba(
            f64::from(val.red) / f64::from(u8::MAX),
            f64::from(val.green) / f64::from(u8::MAX),
            f64::from(val.blue) / f64::from(u8::MAX),
            f64::from(val.alpha) / f64::from(u8::MAX),
        ))
    }
}

impl Deref for Pattern {
    type Target = cairo::Pattern;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Solid(ref p) => p,
            Self::Linear(ref p) => p,
        }
    }
}

#[derive(Debug)]
pub enum Indicator {
    Strings(indicator::Strings),
    Circle(indicator::Circle),
    Classic(indicator::Classic),
}

impl Indicator {
    pub fn set_hover(&mut self, hover: bool, xcontext: &XContext) -> Result<()> {
        match self {
            Self::Strings(i) => i.set_hover(hover, xcontext),
            Self::Circle(..) | Self::Classic(..) => Ok(()),
        }
    }

    pub fn is_inside(&mut self, x: f64, y: f64) -> bool {
        match self {
            Self::Strings(i) => i.is_inside(x, y),
            Self::Circle(..) | Self::Classic(..) => false,
        }
    }

    pub async fn handle_events(&mut self) {
        match self {
            Self::Strings(i) => i.handle_events().await,
            Self::Circle(i) => i.handle_events().await,
            Self::Classic(i) => i.handle_events().await,
        }
    }

    pub fn pass_insert(&mut self, s: &str, pasted: bool) {
        match self {
            Self::Strings(i) => i.pass_insert(s, pasted),
            Self::Circle(i) => i.pass_insert(s, pasted),
            Self::Classic(i) => i.pass_insert(s, pasted),
        }
    }

    pub fn pass_clear(&mut self) {
        match self {
            Self::Strings(i) => i.pass_clear(),
            Self::Circle(i) => i.pass_clear(),
            Self::Classic(i) => i.pass_clear(),
        }
    }

    pub fn pass_delete(&mut self, word: bool) {
        match self {
            Self::Strings(i) => i.pass_delete(word),
            Self::Circle(i) => i.pass_delete(),
            Self::Classic(i) => i.pass_delete(),
        }
    }

    pub fn move_visually(&mut self, direction: indicator::Direction, word: bool) {
        match self {
            Self::Strings(i) => i.move_visually(direction, word),
            Self::Circle(..) | Self::Classic(..) => {}
        }
    }

    pub fn set_cursor(&mut self, x: f64, y: f64) -> bool {
        match self {
            Self::Strings(i) => i.set_cursor(x, y),
            Self::Circle(..) | Self::Classic(..) => false,
        }
    }

    // TODO
    pub fn has_plaintext(&self) -> bool {
        match self {
            Self::Strings(..) => true,
            Self::Circle(..) | Self::Classic(..) => false,
        }
    }

    // TODO
    pub fn toggle_plaintext(&mut self) {
        match self {
            Self::Strings(i) => i.toggle_plaintext(),
            Self::Circle(..) | Self::Classic(..) => unimplemented!(),
        }
    }

    pub fn into_pass(self) -> Passphrase {
        match self {
            Self::Strings(i) => i.into_pass(),
            Self::Circle(i) => i.into_pass(),
            Self::Classic(i) => i.into_pass(),
        }
    }

    pub fn paint(&self, cr: &cairo::Context) {
        match self {
            Self::Strings(i) => i.paint(cr),
            Self::Circle(i) => i.paint(cr),
            Self::Classic(i) => i.paint(cr),
        }
    }

    pub fn set_painted(&mut self) {
        match self {
            Self::Strings(i) => i.set_painted(),
            Self::Circle(i) => i.set_painted(),
            Self::Classic(i) => i.set_painted(),
        }
    }

    pub fn set_next_frame(&mut self) {
        match self {
            Self::Strings(..) | Self::Classic(..) => {}
            Self::Circle(i) => i.set_next_frame(),
        }
    }

    pub fn repaint(&self, cr: &cairo::Context, bg: &Pattern) {
        match self {
            Self::Strings(i) => i.repaint(cr, bg),
            Self::Circle(i) => i.repaint(cr, bg),
            Self::Classic(i) => i.repaint(cr, bg),
        }
    }

    pub fn for_width(&mut self, width: f64) {
        match self {
            Self::Strings(i) => i.for_width(width),
            Self::Circle(..) => {} // TODO
            Self::Classic(i) => i.for_width(width),
        }
    }
}

impl Deref for Indicator {
    type Target = indicator::Base;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Strings(i) => i,
            Self::Circle(i) => i,
            Self::Classic(i) => i,
        }
    }
}

impl DerefMut for Indicator {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Strings(i) => i,
            Self::Circle(i) => i,
            Self::Classic(i) => i,
        }
    }
}

#[derive(Debug)]
pub struct Rectangle {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug)]
pub enum Label {
    TextLabel(TextLabel),
    ClipboardLabel(ClipboardLabel),
}

impl Deref for Label {
    type Target = Rectangle;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::TextLabel(i) => &i.rectangle,
            Self::ClipboardLabel(i) => &i.rectangle,
        }
    }
}

impl DerefMut for Label {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::TextLabel(i) => &mut i.rectangle,
            Self::ClipboardLabel(i) => &mut i.rectangle,
        }
    }
}

impl Label {
    pub fn calc_extents(&mut self, textwidth_req: Option<u32>, compact: bool) {
        match self {
            Self::TextLabel(l) => l.calc_extents(textwidth_req, compact),
            Self::ClipboardLabel(..) => {}
        }
    }
    pub fn paint(&self, cr: &cairo::Context) {
        match self {
            Self::TextLabel(l) => l.paint(cr),
            Self::ClipboardLabel(l) => l.paint(cr),
        }
    }
    pub fn cairo_context_changed(&self, cr: &cairo::Context) {
        match self {
            Self::TextLabel(l) => l.cairo_context_changed(cr),
            Self::ClipboardLabel(..) => {}
        }
    }
}

#[derive(Debug)]
pub struct ClipboardLabel {
    rectangle: Rectangle,
    foreground: Pattern,
}

impl ClipboardLabel {
    pub fn new(foreground: Pattern, text_height: f64) -> Self {
        Self {
            rectangle: Rectangle {
                x: 0.0,
                y: 0.0,
                height: text_height,
                width: text_height * 0.83,
            },
            foreground,
        }
    }
    pub fn paint(&self, cr: &cairo::Context) {
        cr.save().unwrap();
        cr.translate(self.rectangle.x, self.rectangle.y);

        let dot = self.rectangle.height / 18.0;
        let line_width = dot * 1.5;
        let small_height =
            ((self.rectangle.width - 4.0 * dot - 2.0 * line_width) * 0.8).max(2.0 * dot);
        cr.rectangle(0.0, 0.0, self.rectangle.width, self.rectangle.height);
        cr.rectangle(
            line_width,
            0.0,
            self.rectangle.width - 2.0 * line_width,
            small_height,
        );
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.clip();

        let y_offset = dot;
        Button::rounded_rectangle(
            cr,
            2.0 * dot,
            2.0 * dot,
            line_width / 2.0,
            line_width / 2.0 + y_offset,
            self.rectangle.width - line_width,
            self.rectangle.height - line_width - y_offset,
        );
        cr.set_source(&self.foreground).unwrap();
        cr.set_line_width(line_width);
        cr.stroke().unwrap();

        cr.reset_clip();
        let small_width = self.rectangle.width - 4.0 * dot - 3.0 * line_width;
        cr.rectangle(
            line_width + dot * 2.0 + line_width / 2.0,
            line_width / 2.0,
            small_width,
            small_height - line_width,
        );
        cr.stroke().unwrap();

        cr.restore().unwrap();
    }
}

#[derive(Debug)]
pub struct TextLabel {
    rectangle: Rectangle,
    xoff: f64,
    yoff: f64,
    foreground: Pattern,
    pub layout: pango::Layout,
}

impl TextLabel {
    pub fn new(foreground: Pattern, layout: pango::Layout) -> Self {
        Self {
            rectangle: Rectangle {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            xoff: 0.0,
            yoff: 0.0,
            foreground,
            layout,
        }
    }

    pub fn calc_extents(&mut self, textwidth_req: Option<u32>, compact: bool) {
        let mut rect = if compact {
            self.layout.pixel_extents().0
        } else {
            self.layout.pixel_extents().1
        };
        debug!("label rect: {:?}", rect);
        let mut width: u32 = rect.width.try_into().unwrap();
        let mut height: u32 = rect.height.try_into().unwrap();

        if let Some(textwidth_req) = textwidth_req {
            if width > textwidth_req {
                debug!("width: {} > textwidth_req: {}", width, textwidth_req);
                while width > textwidth_req {
                    width /= 2;
                    height *= 2;
                    if height >= width {
                        debug!("height: {} > width: {}", height, width);
                        width *= 2;
                        break;
                    }
                }
                let adjusted_width = width.max(textwidth_req);
                debug!("adjusted width: {}", adjusted_width);
                self.layout
                    .set_width(i32::try_from(adjusted_width).unwrap() * pango::SCALE);
                self.layout.set_wrap(pango::WrapMode::WordChar);
                rect = if compact {
                    self.layout.pixel_extents().0
                } else {
                    self.layout.pixel_extents().1
                };
            }
        }

        self.xoff = f64::from(rect.x);
        self.yoff = f64::from(rect.y);
        self.rectangle.width = f64::from(rect.width);
        self.rectangle.height = f64::from(rect.height);
    }

    pub fn paint(&self, cr: &cairo::Context) {
        cr.save().unwrap();
        cr.translate(self.rectangle.x, self.rectangle.y);
        cr.set_source(&self.foreground).unwrap();
        // TODO am I doin right?
        cr.move_to(-self.xoff, -self.yoff);

        pangocairo::show_layout(cr, &self.layout);
        // DEBUG
        //cr.set_operator(cairo::Operator::Source);
        //cr.rectangle(0.0, 0.0, self.width, self.height);
        //cr.set_source_rgb(0.0, 0.0, 0.0);
        //cr.set_line_width(1.0);
        //cr.stroke();

        cr.restore().unwrap();
    }

    pub fn cairo_context_changed(&self, cr: &cairo::Context) {
        pangocairo::update_layout(cr, &self.layout);
        self.layout.context_changed();
    }
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Button {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    pressed: bool,
    hover: bool,
    dirty: bool,
    border_pattern: Pattern,
    border_pattern_pressed: Pattern,
    vertical_spacing: f64,
    horizontal_spacing: f64,
    interior_width: f64,
    interior_height: f64,
    label: Label,
    background: Option<Pattern>,
    bg_pressed: Option<Pattern>,
    bg_hover: Option<Pattern>,
    config: config::Button,
    toggled: bool,
}

impl Button {
    pub fn new(config: config::Button, label: Label, text_height: f64) -> Self {
        let vertical_spacing = config.vertical_spacing.unwrap_or(text_height / 3.0).round();
        let horizontal_spacing = if matches!(label, Label::ClipboardLabel(_)) {
            config
                .horizontal_spacing
                .unwrap_or(text_height / 2.0)
                .round()
        } else {
            config.horizontal_spacing.unwrap_or(text_height).round()
        };
        debug!(
            "button vertical_spacing: {}, horizontal_spacing: {}, border_width: {}",
            vertical_spacing, horizontal_spacing, config.border_width
        );
        let mut me = Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            pressed: false,
            hover: false,
            dirty: true,
            border_pattern: config.border_color.into(),
            border_pattern_pressed: config.border_color_pressed.into(),
            interior_width: 0.0,
            interior_height: 0.0,
            vertical_spacing,
            horizontal_spacing,
            label,
            background: None,
            bg_pressed: None,
            bg_hover: None,
            config,
            toggled: false,
        };
        me.calc_extents();
        me
    }

    pub fn toggle(&mut self) {
        self.toggled = !self.toggled;
        self.dirty = true;
    }

    fn clear(&self, cr: &cairo::Context, bg: &Pattern) {
        cr.rectangle(
            self.x - 1.0,
            self.y - 1.0,
            self.width + 2.0,
            self.height + 2.0,
        );
        cr.save().unwrap();
        cr.set_operator(cairo::Operator::Source);
        cr.set_source(bg).unwrap();
        cr.fill().unwrap();
        cr.restore().unwrap();
    }

    fn calc_extents(&mut self) {
        self.label.calc_extents(None, false);
        self.interior_width = self.label.width + (2.0 * self.horizontal_spacing);
        self.interior_height = self.label.height + (2.0 * self.vertical_spacing);
        self.calc_total_extents();
    }

    fn calc_total_extents(&mut self) {
        self.width = self.interior_width + 2.0 * self.config.border_width;
        self.height = self.interior_height + 2.0 * self.config.border_width;

        // TODO placement
        let fill_height = self.height - self.config.border_width;
        self.background = Some(Pattern::get_pattern(
            fill_height,
            self.config.background,
            self.config.background_stop,
        ));
        self.bg_pressed = Some(Pattern::get_pattern(
            fill_height,
            self.config.background_pressed,
            self.config.background_pressed_stop,
        ));
        self.bg_hover = Some(Pattern::get_pattern(
            fill_height,
            self.config.background_hover,
            self.config.background_hover_stop,
        ));
    }

    fn calc_label_position(&mut self) {
        self.label.x = (self.width - self.label.width) / 2.0;
        self.label.y = (self.height - self.label.height) / 2.0;
        debug!(
            "button/label: label.x: {}, label.y: {}",
            self.label.x, self.label.y
        );
    }

    pub fn is_inside(&self, x: f64, y: f64) -> bool {
        x >= self.x + self.config.border_width
            && x < self.x + self.width - self.config.border_width
            && y >= self.y + self.config.border_width
            && y < self.y + self.height - self.config.border_width
    }

    pub fn set_hover(&mut self, hover: bool) {
        self.dirty = self.dirty || self.hover != hover;
        self.hover = hover;
    }

    pub fn set_pressed(&mut self, pressed: bool) {
        self.dirty = self.dirty || self.pressed != pressed;
        self.pressed = pressed;
    }

    // from https://www.cairographics.org/cookbook/roundedrectangles/
    fn rounded_rectangle(
        cr: &cairo::Context,
        mut radius_x: f64,
        mut radius_y: f64,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        const ARC_TO_BEZIER: f64 = 0.552_284_75;
        trace!("rounded_rectangle x: {}, y: {}, w: {}, h: {}", x, y, w, h);
        // from mono moonlight aka mono silverlight
        // test limits (without using multiplications)
        // http://graphics.stanford.edu/courses/cs248-98-fall/Final/q1.html
        if radius_x > w - radius_x {
            radius_x = w / 2.0;
        }
        if radius_y > h - radius_y {
            radius_y = h / 2.0;
        }

        // approximate (quite close) the arc using a bezier curve
        let c1 = ARC_TO_BEZIER * radius_x;
        let c2 = ARC_TO_BEZIER * radius_y;

        cr.new_path();
        cr.move_to(x + radius_x, y);
        cr.rel_line_to(w - 2.0 * radius_x, 0.0);
        cr.rel_curve_to(c1, 0.0, radius_x, c2, radius_x, radius_y);
        cr.rel_line_to(0.0, h - 2.0 * radius_y);
        cr.rel_curve_to(0.0, c2, c1 - radius_x, radius_y, -radius_x, radius_y);
        cr.rel_line_to(-w + 2.0 * radius_x, 0.0);
        cr.rel_curve_to(-c1, 0.0, -radius_x, -c2, -radius_x, -radius_y);
        cr.rel_line_to(0.0, -h + 2.0 * radius_y);
        cr.rel_curve_to(0.0, -c2, radius_x - c1, -radius_y, radius_x, -radius_y);
        cr.close_path();
    }

    pub fn set_painted(&mut self) {
        self.dirty = false;
    }

    pub fn paint(&self, cr: &cairo::Context) {
        trace!("button paint start");
        cr.save().unwrap();
        cr.translate(self.x, self.y);

        // "Note that while stroking the path transfers the source for half of the line width on
        // each side of the path, filling a path fills directly up to the edge of the path and no
        // further." We use stroke below so modifying accordingly.
        let x = self.config.border_width / 2.0;
        let y = self.config.border_width / 2.0;
        let width = self.width - self.config.border_width;
        let height = self.height - self.config.border_width;
        Self::rounded_rectangle(
            cr,
            self.config.radius_x,
            self.config.radius_y,
            x,
            y,
            width,
            height,
        );

        let bg = if self.pressed && self.hover {
            &self.bg_pressed
        } else if self.hover {
            &self.bg_hover
        } else if self.toggled {
            &self.bg_pressed
        } else {
            &self.background
        };
        cr.set_source(bg.as_ref().unwrap()).unwrap();
        cr.fill_preserve().unwrap();

        if self.config.border_width > 0.0 {
            if std::ptr::eq(bg, &self.bg_pressed) {
                cr.set_source(&self.border_pattern_pressed).unwrap();
            } else {
                cr.set_source(&self.border_pattern).unwrap();
            }
            cr.set_line_width(self.config.border_width);
            cr.stroke().unwrap();
        }

        if self.pressed && self.hover {
            cr.translate(
                self.config.pressed_adjustment_x,
                self.config.pressed_adjustment_y,
            );
        }
        self.label.paint(cr);

        cr.restore().unwrap();
    }
}

fn balance_button_extents(button1: &mut Button, button2: &mut Button) {
    button1.interior_width = button1.interior_width.max(button2.interior_width);
    button2.interior_width = button1.interior_width;
    button1.interior_height = button1.interior_height.max(button2.interior_height);
    button2.interior_height = button1.interior_height;
    button1.calc_total_extents();
    button2.calc_total_extents();
}

pub fn setlocale() {
    let locale = unsafe { libc::setlocale(LC_ALL, b"\0".as_ptr().cast()) };
    if locale.is_null() {
        warn!("setlocale failed");
        return;
    }
    debug!("locale: {}", unsafe {
        CStr::from_ptr(locale).to_str().unwrap()
    });
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Dialog {
    background: Pattern,
    background_original: Rgba,
    buttons: Vec<Button>,
    labels: Vec<Label>,
    pub indicator: Indicator,
    width: f64,
    height: f64,
    mouse_middle_pressed: bool,
    input_timeout_duration: Option<Duration>,
    input_timeout: Option<Pin<Box<Sleep>>>,
    debug: bool,
    pub uses_cursor: bool,
    button_pressed: bool,
    transparency: bool,
    dirty: bool,
}

impl Dialog {
    #[allow(clippy::too_many_lines)]
    pub fn new(
        config: config::Dialog,
        screen: &xproto::Screen,
        cr: &cairo::Context,
        label: Option<&str>,
        debug: bool,
    ) -> Result<Self> {
        if let Some(font_file) = config.font_file {
            unsafe {
                let fc = fontconfig::FcConfigCreate();
                if fontconfig::FcConfigSetCurrent(fc) == 0 {
                    bail!("FcConfigSetCurrent failed");
                }
                if fontconfig::FcConfigAppFontAddFile(
                    std::ptr::null_mut(),
                    font_file.as_ptr().cast(),
                ) == 0
                {
                    bail!("Could not load font file: {}", font_file.to_string_lossy());
                }
            }
        }

        if let Some(scale) = config.scale {
            if scale <= 0.0 {
                bail!("invalid scale {}", scale);
            }
            cr.scale(scale, scale);
        } else if screen.height_in_pixels > 1080 {
            let scale = f64::from(screen.height_in_pixels) / 1080.0;
            cr.scale(scale, scale);
        }

        let pango_context = pangocairo::create_context(cr).unwrap();

        let language = pango::Language::default();
        debug!("language {}", language.to_string());
        pango_context.set_language(&language);
        debug!("base_dir {}", pango_context.base_dir());

        if let Some(font) = config.font {
            let mut font_desc = pango::FontDescription::from_string(&font);
            debug!("font request: {}", font_desc.to_string());
            if font_desc.size() == 0 {
                debug!("setting font size to default 11");
                font_desc.set_size(11 * pango::SCALE);
            }
            pango_context.set_font_description(&font_desc);
        }

        if log_enabled!(log::Level::Debug) {
            let closest_font = pango_context
                .load_font(&pango_context.font_description().unwrap())
                .unwrap()
                .describe()
                .map_or_else(|| "<no name>".into(), |f| f.to_string());
            debug!("closest font: {}", closest_font);
        }

        let metrics = pango_context.metrics(None, None).unwrap();
        let text_height = f64::from(metrics.ascent() + metrics.descent()) / f64::from(pango::SCALE);
        let text_height = cr
            .user_to_device_distance(0.0, text_height)
            .expect("cairo user_to_device_distance")
            .1
            .ceil();
        debug!("text height: {}", text_height);

        let label_layout = pango::Layout::new(&pango_context);
        label_layout.set_text(label.unwrap_or(&config.label));

        let label = Label::TextLabel(TextLabel::new(config.foreground.into(), label_layout));

        let ok_layout = pango::Layout::new(&pango_context);
        let cancel_layout = pango::Layout::new(&pango_context);

        ok_layout.set_text(&config.ok_button.label);
        let ok_label = Label::TextLabel(TextLabel::new(
            config.ok_button.foreground.into(),
            ok_layout,
        ));
        cancel_layout.set_text(&config.cancel_button.label);
        let cancel_label = Label::TextLabel(TextLabel::new(
            config.cancel_button.foreground.into(),
            cancel_layout,
        ));

        let mut ok_button = Button::new(config.ok_button.button, ok_label, text_height);
        let mut cancel_button = Button::new(config.cancel_button.button, cancel_label, text_height);
        balance_button_extents(&mut ok_button, &mut cancel_button);

        // TODO
        let uses_cursor = matches!(
            config.indicator.indicator_type,
            IndicatorType::Strings { .. }
        );

        let mut indicator = match config.indicator.indicator_type {
            IndicatorType::Strings { strings } => {
                let indicator_layout = pango::Layout::new(&pango_context);
                Indicator::Strings(indicator::Strings::new(
                    config.indicator.common,
                    strings,
                    indicator_layout,
                    debug,
                    text_height,
                ))
            }
            IndicatorType::Classic { classic } => Indicator::Classic(indicator::Classic::new(
                config.indicator.common,
                classic,
                text_height,
                debug,
            )),
            IndicatorType::Circle { circle } => Indicator::Circle(indicator::Circle::new(
                config.indicator.common,
                circle,
                text_height,
                debug,
            )),
        };

        let mut labels = Vec::with_capacity(2);
        labels.push(label);
        let mut buttons = Vec::with_capacity(3);
        buttons.push(ok_button);
        buttons.push(cancel_button);
        let mut components = Components {
            plaintext_config: Some(config.plaintext_button),
            clipboard_config: Some(config.clipboard_button),
            indicator_label_foreground: Some(config.indicator_label_foreground),
            indicator_label_text: config.indicator_label,
            buttons,
            text_height,
            labels,
            pango_context,
        };

        debug!(
            "layout: vertical_spacing: {}, horizontal_spacing: {}",
            config.layout_opts.horizontal_spacing(text_height),
            config.layout_opts.vertical_spacing(text_height)
        );
        let (width, height) = config.layout_opts.layout.get_fn()(
            &config.layout_opts,
            &mut components,
            &mut indicator,
        );

        let mut buttons = components.buttons;

        for b in &mut buttons {
            b.calc_label_position();
        }

        Ok(Self {
            indicator,
            buttons,
            labels: components.labels,
            width,
            height,
            mouse_middle_pressed: false,
            background: config.background.into(),
            background_original: config.background,
            input_timeout_duration: config.input_timeout.map(Duration::from_secs),
            input_timeout: None,
            debug,
            uses_cursor,
            button_pressed: false,
            transparency: true,
            dirty: false,
        })
    }

    pub fn set_transparency(&mut self, enable: bool) {
        if self.transparency == enable {
            debug!("set_transparency: status not changed");
            return;
        }
        if self.background_original.alpha == u8::MAX {
            debug!("set_transparency: original background not transparent");
            return;
        }
        debug!("set_transparency: {}", enable);
        self.dirty = true;
        self.transparency = enable;
        if enable {
            self.background = self.background_original.into();
        } else {
            let mut background = self.background_original;
            background.alpha = u8::MAX;
            self.background = background.into();
        }
    }

    pub fn set_next_frame(&mut self) {
        self.indicator.set_next_frame();
    }

    pub fn set_painted(&mut self) {
        trace!("set_painted");
        self.indicator.set_painted();
        for b in &mut self.buttons {
            b.set_painted();
        }
        self.dirty = false;
    }

    pub fn dirty(&self) -> bool {
        if self.indicator.dirty() {
            return true;
        }
        for b in &self.buttons {
            if b.dirty {
                return true;
            }
        }
        self.dirty
    }

    pub fn repaint(&self, cr: &cairo::Context) {
        if self.dirty {
            return self.init(cr);
        }

        self.indicator.repaint(cr, &self.background);
        for (i, b) in self.buttons.iter().enumerate() {
            if b.dirty {
                trace!("button {} dirty", i);
                b.clear(cr, &self.background);
                b.paint(cr);
            }
        }
    }

    pub fn window_size(&self, cr: &cairo::Context) -> (u16, u16) {
        let size = cr
            .user_to_device_distance(self.width, self.height)
            .expect("cairo user_to_device_distance");
        (size.0.round() as u16, size.1.round() as u16)
    }

    pub fn init(&self, cr: &cairo::Context) {
        // TODO can I preserve antialiasing without clearing the image first?
        cr.set_operator(cairo::Operator::Source);
        cr.set_source(&self.background).unwrap();
        cr.paint().unwrap();
        cr.set_operator(cairo::Operator::Over);
        self.paint(cr);
    }

    fn paint(&self, cr: &cairo::Context) {
        trace!("paint");
        for l in &self.labels {
            l.paint(cr);
        }
        self.indicator.paint(cr);
        for b in &self.buttons {
            b.paint(cr);
        }
    }

    pub fn init_events(&mut self) {
        self.indicator.init_timeouts();
        self.input_timeout = Some(Box::pin(sleep(
            self.input_timeout_duration
                .unwrap_or_else(|| Duration::from_secs(0)),
        )));
    }

    pub async fn handle_events(&mut self) -> Action {
        tokio::select! {
            _ = self.input_timeout.as_mut().unwrap(), if self.input_timeout_duration.is_some() => {
                info!("input timeout");
                Action::Cancel
            }
            _ = self.indicator.handle_events(), if self.indicator.requests_events() => {
                Action::Nothing
            }
        }
    }

    pub fn handle_motion(&mut self, x: f64, y: f64, xcontext: &XContext) -> Result<()> {
        let mut found = false;
        for b in &mut self.buttons {
            if found {
                trace!("set_hover: false");
                b.set_hover(false);
            } else if b.is_inside(x, y) {
                trace!("set_hover: {}", self.button_pressed == b.pressed);
                b.set_hover(self.button_pressed == b.pressed);
                found = true;
            } else {
                trace!("set_hover: false");
                b.set_hover(false);
            }
        }
        if !found && self.indicator.is_inside(x, y) {
            self.indicator.set_hover(true, xcontext)?;
        } else {
            self.indicator.set_hover(false, xcontext)?;
        };
        Ok(())
    }

    fn cairo_context_changed(&mut self, cr: &cairo::Context) {
        for l in &mut self.labels {
            l.cairo_context_changed(cr);
        }
        for b in &mut self.buttons {
            b.label.cairo_context_changed(cr);
        }
    }

    pub fn resize(&mut self, cr: &cairo::Context, width: u16, height: u16, surface_cleared: bool) {
        cr.set_operator(cairo::Operator::Source);
        cr.set_source(&self.background).unwrap();

        // TODO put to clear()
        if surface_cleared {
            // clear the whole buffer
            cr.paint().unwrap();
        } else {
            // use the translation matrix for the previous window size to clear the previously used
            // area
            // TODO
            cr.rectangle(
                -1.0,
                -1.0,
                self.width as f64 + 2.0,
                self.height as f64 + 2.0,
            );
            cr.fill().unwrap();
        }
        cr.set_operator(cairo::Operator::Over);

        let mut m = cr.matrix();

        let (dialog_width, dialog_height) = self.window_size(cr);
        m.x0 = if width > dialog_width {
            // floor to pixels
            f64::from((width - dialog_width) / 2)
        } else {
            0.0
        };
        m.y0 = if height > dialog_height {
            // floor to pixels
            f64::from((height - dialog_height) / 2)
        } else {
            0.0
        };

        cr.set_matrix(m);

        self.cairo_context_changed(cr);

        self.paint(cr);
    }

    pub fn handle_button_press(
        &mut self,
        button: xproto::ButtonIndex,
        x: f64,
        y: f64,
        isrelease: bool,
        xcontext: &mut XContext,
    ) -> Result<Action> {
        if let Some(timeout) = self.input_timeout_duration {
            self.input_timeout
                .as_mut()
                .unwrap()
                .as_mut()
                .reset(Instant::now().checked_add(timeout).unwrap());
        }

        let action = if !isrelease && button == xproto::ButtonIndex::M2 {
            self.mouse_middle_pressed = true;
            Action::Nothing
        } else if self.mouse_middle_pressed && button == xproto::ButtonIndex::M2 {
            self.mouse_middle_pressed = false;
            if x >= 0.0 && x < self.width as f64 && y >= 0.0 && y < self.height as f64 {
                Action::PastePrimary
            } else {
                Action::Nothing
            }
        } else if button == xproto::ButtonIndex::M1 {
            self.handle_mouse_left_button_press(x, y, isrelease)
        } else {
            trace!("not the left mouse button");
            Action::Nothing
        };

        match action {
            Action::Ok => return Ok(Action::Ok),
            Action::Cancel => return Ok(Action::Cancel),
            Action::PastePrimary => {
                xcontext.paste_primary()?;
            }
            Action::PasteClipboard => {
                xcontext.paste_clipboard()?;
            }
            Action::PlainText => {
                self.indicator.toggle_plaintext();
                self.buttons[3].toggle();
            }
            Action::Nothing => {}
        }

        Ok(Action::Nothing)
    }

    // Return true iff dialog should be repainted
    fn handle_mouse_left_button_press(&mut self, x: f64, y: f64, release: bool) -> Action {
        if release {
            self.button_pressed = false;
            for (i, b) in self.buttons.iter_mut().enumerate() {
                if b.pressed {
                    b.set_pressed(false);
                    if b.is_inside(x, y) {
                        trace!("release inside button {}", i);
                        return Components::ACTIONS[i];
                    }
                    return Action::Nothing;
                }
            }
        } else {
            let inside = self.indicator.set_cursor(x, y);
            if inside {
                return Action::Nothing;
            }
            for (i, b) in self.buttons.iter_mut().enumerate() {
                if b.is_inside(x, y) {
                    trace!("inside button {}", i);
                    b.set_pressed(true);
                    self.button_pressed = true;
                    return Action::Nothing;
                }
            }
        }
        Action::Nothing
    }

    fn get_secure_utf8_do(keyboard: &Keyboard, key_press: Keycode, composed: bool) -> SecBuf<u8> {
        let mut buf = SecBuf::new(vec![0; 60]);
        buf.len = if composed {
            keyboard
                .compose
                .as_ref()
                .unwrap()
                .compose_state_get_utf8(buf.buf.unsecure_mut())
        } else {
            keyboard.key_get_utf8(key_press, buf.buf.unsecure_mut())
        };
        if buf.len > buf.unsecure().len() {
            buf = SecBuf::new(vec![0; buf.len]);
            buf.len = if composed {
                keyboard
                    .compose
                    .as_ref()
                    .unwrap()
                    .compose_state_get_utf8(buf.buf.unsecure_mut())
            } else {
                keyboard.key_get_utf8(key_press, buf.buf.unsecure_mut())
            };
        }
        buf
    }

    pub fn handle_key_press(&mut self, key: Keycode, xcontext: &mut XContext) -> Result<Action> {
        if let Some(timeout) = self.input_timeout_duration {
            self.input_timeout
                .as_mut()
                .unwrap()
                .as_mut()
                .reset(Instant::now().checked_add(timeout).unwrap());
        }

        let keyboard = &xcontext.keyboard;
        let mut key_sym = keyboard.key_get_one_sym(key);
        if self.debug {
            debug!("key: {:#x}, key_sym {:#x}", key, key_sym);
        }

        let mut composed = false;
        if let Some(ref compose) = keyboard.compose {
            if compose.state_feed(key_sym) == xkb_compose_feed_result::XKB_COMPOSE_FEED_ACCEPTED {
                match compose.state_get_status() {
                    xkb_compose_status::XKB_COMPOSE_NOTHING => {}
                    xkb_compose_status::XKB_COMPOSE_COMPOSING => {
                        return Ok(Action::Nothing);
                    }
                    xkb_compose_status::XKB_COMPOSE_COMPOSED => {
                        key_sym = compose.state_get_one_sym();
                        composed = true;
                    }
                    xkb_compose_status::XKB_COMPOSE_CANCELLED => {
                        compose.state_reset();
                        return Ok(Action::Nothing);
                    }
                    _ => unreachable!(),
                }
            }
        }

        let ctrl = xcontext.keyboard.mod_name_is_active(
            keyboard::names::XKB_MOD_NAME_CTRL,
            keyboard::xkb_state_component::XKB_STATE_MODS_EFFECTIVE,
        );

        let mut matched = true;
        let mut action = Action::Nothing;
        match key_sym {
            keysyms::XKB_KEY_Return | keysyms::XKB_KEY_KP_Enter => {
                action = Action::Ok;
            }
            keysyms::XKB_KEY_j | keysyms::XKB_KEY_m if ctrl => {
                action = Action::Ok;
            }
            keysyms::XKB_KEY_Escape => {
                action = Action::Cancel;
            }
            keysyms::XKB_KEY_BackSpace => self.indicator.pass_delete(ctrl),
            keysyms::XKB_KEY_h if ctrl => self.indicator.pass_delete(false),
            keysyms::XKB_KEY_u if ctrl => self.indicator.pass_clear(),
            keysyms::XKB_KEY_v if ctrl => {
                xcontext.paste_clipboard()?;
            }
            keysyms::XKB_KEY_Left => self.indicator.move_visually(indicator::Direction::Left, ctrl),
            keysyms::XKB_KEY_Right => self.indicator.move_visually(indicator::Direction::Right, ctrl),
            keysyms::XKB_KEY_Insert
                if xcontext.keyboard.mod_name_is_active(
                    keyboard::names::XKB_MOD_NAME_SHIFT,
                    keyboard::xkb_state_component::XKB_STATE_MODS_EFFECTIVE,
                ) =>
            {
                xcontext.paste_primary()?;
            }
            _ => {
                matched = false;
            }
        };
        key_sym.zeroize();
        if matched {
            return Ok(action);
        }

        let buf = Self::get_secure_utf8_do(&xcontext.keyboard, key, composed);
        let s = unsafe { std::str::from_utf8_unchecked(buf.unsecure()) };
        if !s.is_empty() {
            self.indicator.pass_insert(s, false);
            return Ok(Action::Nothing);
        }
        Ok(Action::Nothing)
    }
}
