use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::ffi::CStr;
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use libc::LC_ALL;
use log::{debug, info, log_enabled, trace, warn};
use pango::FontExt as _;
use tokio::time::{sleep, Instant, Sleep};
use x11rb::protocol::xproto;
use zeroize::Zeroize;

use crate::backbuffer::FrameId;
use crate::config;
use crate::config::{IndicatorType, Rgba};
use crate::errors::Result;
use crate::event::{Event, Keypress, XContext};
use crate::keyboard;
use crate::secret::Passphrase;

pub mod indicator;
pub mod layout;

#[derive(Clone, Copy)]
pub enum Action {
    NoAction,
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
    font_desc: pango::FontDescription,
    pango_context: pango::Context,
    buttons: Vec<Button>,
    text_height: u32,
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
                self.text_height as f64,
            ));
            self.buttons
                .push(Button::new(config.button, clipboard_label));
        }
        &mut self.buttons[2]
    }

    fn plaintext(&mut self) -> &mut Button {
        if self.buttons.get_mut(3).is_none() {
            debug!("creating plaintext button");
            // TODO use own config
            let config = self.plaintext_config.take().unwrap();
            let layout = pango::Layout::new(&self.pango_context);
            layout.set_font_description(Some(&self.font_desc));
            layout.set_text(&config.label);
            let label = Label::TextLabel(TextLabel::new(config.foreground.into(), layout));
            self.buttons.push(Button::new(config.button, label));
        }
        &mut self.buttons[3]
    }

    fn indicator_label(&mut self) -> &mut Label {
        if self.labels.get_mut(1).is_none() {
            debug!("creating indicator label");
            let indicator_layout = pango::Layout::new(&self.pango_context);
            indicator_layout.set_font_description(Some(&self.font_desc));
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
    fn get_pattern(fill_height: f64, start: Rgba, end: Option<Rgba>) -> Self {
        if let Some(end) = end {
            let grad = cairo::LinearGradient::new(0.0, 0.0, 0.0, fill_height);
            grad.add_color_stop_rgba(
                0.0,
                start.red as f64 / u8::MAX as f64,
                start.green as f64 / u8::MAX as f64,
                start.blue as f64 / u8::MAX as f64,
                start.alpha as f64 / u8::MAX as f64,
            );
            grad.add_color_stop_rgba(
                1.0,
                end.red as f64 / u8::MAX as f64,
                end.green as f64 / u8::MAX as f64,
                end.blue as f64 / u8::MAX as f64,
                end.alpha as f64 / u8::MAX as f64,
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
            val.red as f64 / u8::MAX as f64,
            val.green as f64 / u8::MAX as f64,
            val.blue as f64 / u8::MAX as f64,
            val.alpha as f64 / u8::MAX as f64,
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
    // TODO
    pub fn has_plaintext(&self) -> bool {
        match self {
            Self::Strings(..) => true,
            Self::Circle(..) => false,
            Self::Classic(..) => false,
        }
    }

    // TODO
    pub fn toggle_plaintext(&mut self) {
        match self {
            Self::Strings(i) => i.toggle_plaintext(),
            Self::Circle(..) => unimplemented!(),
            Self::Classic(..) => unimplemented!(),
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

    pub fn passphrase_updated(&mut self) -> bool {
        match self {
            Self::Strings(i) => i.passphrase_updated(),
            Self::Circle(i) => i.passphrase_updated(),
            Self::Classic(i) => i.passphrase_updated(),
        }
    }

    pub fn set_painted(&mut self, serial: FrameId) {
        match self {
            Self::Strings(i) => i.set_painted(),
            Self::Circle(i) => i.set_painted(serial),
            Self::Classic(i) => i.set_painted(),
        }
    }

    pub fn on_displayed(&mut self, serial: FrameId) -> bool {
        match self {
            Self::Strings(..) => false,
            Self::Circle(i) => i.on_displayed(serial),
            Self::Classic(..) => false,
        }
    }

    pub fn update(&self, cr: &cairo::Context, bg: &Pattern) {
        match self {
            Self::Strings(i) => i.update(cr, bg),
            Self::Circle(i) => i.update(cr, bg),
            Self::Classic(i) => i.update(cr, bg),
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
                width: (text_height * 0.85).round(),
            },
            foreground,
        }
    }
    pub fn paint(&self, cr: &cairo::Context) {
        cr.save();
        cr.translate(self.rectangle.x, self.rectangle.y);

        let line_width = 1.5;
        let barely_noticeable = (self.rectangle.width / 10.0).floor().max(1.0);
        let small_height = ((self.rectangle.width - 4.0 * barely_noticeable - 2.0 * line_width)
            * 0.8)
            .round()
            .max(2.0);
        cr.rectangle(0.0, 0.0, self.rectangle.width, self.rectangle.height);
        cr.rectangle(
            line_width,
            0.0,
            self.rectangle.width - 2.0 * line_width,
            small_height,
        );
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.clip();

        let y_offset = barely_noticeable;
        Button::rounded_rectangle(
            cr,
            2.0,
            2.0,
            line_width / 2.0,
            line_width / 2.0 + y_offset,
            self.rectangle.width - line_width,
            self.rectangle.height - line_width - y_offset,
        );
        cr.set_source(&self.foreground);
        cr.set_line_width(line_width);
        cr.stroke();

        cr.reset_clip();
        let small_width = self.rectangle.width - 4.0 * barely_noticeable - 3.0 * line_width;
        cr.rectangle(
            line_width + barely_noticeable * 2.0 + line_width / 2.0,
            line_width / 2.0,
            small_width,
            small_height - line_width,
        );
        cr.stroke();

        cr.restore();
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
            self.layout.get_pixel_extents().0
        } else {
            self.layout.get_pixel_extents().1
        };
        trace!("label rect: {:?}", rect);
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
                    self.layout.get_pixel_extents().0
                } else {
                    self.layout.get_pixel_extents().1
                };
            }
        }

        self.xoff = rect.x as f64;
        self.yoff = rect.y as f64;
        self.rectangle.width = rect.width as f64;
        self.rectangle.height = rect.height as f64;
    }

    pub fn paint(&self, cr: &cairo::Context) {
        cr.save();
        cr.translate(self.rectangle.x, self.rectangle.y);
        cr.set_source(&self.foreground);
        // TODO am I doin right?
        cr.move_to(-self.xoff, -self.yoff);

        pangocairo::show_layout(cr, &self.layout);
        // DEBUG
        //cr.set_operator(cairo::Operator::Source);
        //cr.rectangle(0.0, 0.0, self.width, self.height);
        //cr.set_source_rgb(0.0, 0.0, 0.0);
        //cr.set_line_width(1.0);
        //cr.stroke();

        cr.restore();
    }

    pub fn cairo_context_changed(&self, cr: &cairo::Context) {
        pangocairo::update_layout(cr, &self.layout);
        self.layout.context_changed();
    }
}

#[derive(Debug)]
pub struct Button {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    pressed: bool,
    hover: bool,
    dirty: bool,
    horizontal_spacing: f64,
    vertical_spacing: f64,
    border_width: f64,
    border_pattern: Pattern,
    interior_width: f64,
    interior_height: f64,
    pressed_adjustment_x: f64,
    pressed_adjustment_y: f64,
    radius_x: f64,
    radius_y: f64,
    label: Label,
    background: Option<Pattern>,
    bg_pressed: Option<Pattern>,
    bg_hover: Option<Pattern>,
    config: config::Button,
}

impl Button {
    pub fn new(config: config::Button, label: Label) -> Self {
        let mut me = Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            pressed: false,
            hover: false,
            dirty: true,
            horizontal_spacing: config.horizontal_spacing,
            vertical_spacing: config.vertical_spacing,
            border_width: config.border_width,
            border_pattern: config.border_color.clone().into(), // TODO avoid cloning?
            radius_x: config.radius_x,
            radius_y: config.radius_y,
            interior_width: 0.0,
            interior_height: 0.0,
            pressed_adjustment_x: config.pressed_adjustment_x,
            pressed_adjustment_y: config.pressed_adjustment_y,
            label,
            background: None,
            bg_pressed: None,
            bg_hover: None,
            config,
        };
        me.calc_extents();
        me
    }

    fn clear(&self, cr: &cairo::Context, bg: &Pattern) {
        cr.rectangle(
            self.x - 1.0,
            self.y - 1.0,
            self.width + 2.0,
            self.height + 2.0,
        );
        cr.set_source(bg);
        cr.fill();
    }

    fn calc_extents(&mut self) {
        self.label.calc_extents(None, false);
        self.interior_width = self.label.width + (2.0 * self.horizontal_spacing);
        self.interior_height = self.label.height + (2.0 * self.vertical_spacing);
        self.calc_total_extents();
    }

    fn calc_total_extents(&mut self) {
        self.width = self.interior_width + 2.0 * self.border_width;
        self.height = self.interior_height + 2.0 * self.border_width;

        // TODO placement, avoid cloning
        let fill_height = self.height - self.border_width;
        self.background = Some(Pattern::get_pattern(
            fill_height,
            self.config.background.clone(),
            self.config.background_stop.clone(),
        ));
        self.bg_pressed = Some(Pattern::get_pattern(
            fill_height,
            self.config.background_pressed.clone(),
            self.config.background_pressed_stop.clone(),
        ));
        self.bg_hover = Some(Pattern::get_pattern(
            fill_height,
            self.config.background_hover.clone(),
            self.config.background_hover_stop.clone(),
        ));
    }

    fn calc_label_position(&mut self) {
        self.label.x = ((self.width - self.label.width) / 2.0).floor();
        self.label.y = ((self.height - self.label.height) / 2.0).floor();
    }

    pub fn is_inside(&self, x: f64, y: f64) -> bool {
        x >= self.x + self.border_width
            && x < self.x + self.width - (2.0 * self.border_width)
            && y >= self.y + self.border_width
            && y < self.y + self.height - (2.0 * self.border_width)
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
        // from mono moonlight aka mono silverlight
        // test limits (without using multiplications)
        // http://graphics.stanford.edu/courses/cs248-98-fall/Final/q1.html
        const ARC_TO_BEZIER: f64 = 0.55228475;
        if radius_x > w - radius_x {
            radius_x = (w / 2.0).floor();
        }
        if radius_y > h - radius_y {
            radius_y = (h / 2.0).floor();
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

    pub fn paint(&mut self, cr: &cairo::Context) {
        cr.save();
        cr.translate(self.x, self.y);

        // "Note that while stroking the path transfers the source for half of the line width on
        // each side of the path, filling a path fills directly up to the edge of the path and no
        // further." We use stroke below so modifying accordingly.
        let x = self.border_width / 2.0;
        let y = self.border_width / 2.0;
        let width = self.width - self.border_width;
        let height = self.height - self.border_width;
        Self::rounded_rectangle(cr, self.radius_x, self.radius_y, x, y, width, height);

        let bg = if self.pressed && self.hover {
            &self.bg_pressed
        } else if self.hover {
            &self.bg_hover
        } else {
            &self.background
        };
        cr.set_source(bg.as_ref().unwrap());
        cr.fill_preserve();

        if self.border_width > 0.0 {
            cr.set_source(&self.border_pattern);
            cr.set_line_width(self.border_width);
            cr.stroke();
        }

        if self.pressed && self.hover {
            cr.translate(self.pressed_adjustment_x, self.pressed_adjustment_y);
        }
        self.label.paint(cr);

        cr.restore();
        self.dirty = false;
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
    let locale = unsafe { libc::setlocale(LC_ALL, &'\0' as *const _ as _) };
    if locale.is_null() {
        warn!("setlocale failed");
        return;
    }
    debug!("locale: {}", unsafe {
        CStr::from_ptr(locale).to_str().unwrap()
    });
}

#[derive(Debug)]
pub struct Dialog {
    background: Pattern,
    buttons: Vec<Button>,
    labels: Vec<Label>,
    pub indicator: Indicator,
    width: f64,
    height: f64,
    mouse_middle_pressed: bool,
    input_timeout: Sleep,
    input_timeout_duration: Option<Duration>,
    debug: bool,
}

impl Dialog {
    pub fn new(
        config: config::Dialog,
        screen: &xproto::Screen,
        cr: &cairo::Context,
        label: Option<&str>,
        debug: bool,
    ) -> Result<Self> {
        let pango_context = pangocairo::create_context(&cr).unwrap();

        let dpi = if let Some(dpi) = config.dpi {
            dpi
        } else {
            (screen.height_in_pixels as f64 * 25.4 / screen.height_in_millimeters as f64)
                .max(96.0)
                .round()
        };
        debug!("dpi {}", dpi);
        pangocairo::context_set_resolution(&pango_context, dpi);

        let language = pango::Language::default();
        debug!("language {}", language.to_string());
        pango_context.set_language(&language);

        let font = config.font;
        let font_desc = pango::FontDescription::from_string(&font);

        debug!("font request: {}", font_desc.to_string());
        if log_enabled!(log::Level::Debug) {
            let closest_font = pango_context
                .load_font(&font_desc)
                .unwrap()
                .describe()
                .map(|f| f.to_string())
                .unwrap_or_else(|| "<no name>".into());
            debug!("closest font: {}", closest_font);
        }

        let label_layout = pango::Layout::new(&pango_context);
        label_layout.set_font_description(Some(&font_desc));
        label_layout.set_text(label.unwrap_or(&config.label));
        let (_, text_height) = label_layout.get_pixel_size();
        let text_height: u32 = text_height.try_into().unwrap();

        let label = Label::TextLabel(TextLabel::new(config.foreground.into(), label_layout));

        let ok_layout = pango::Layout::new(&pango_context);
        ok_layout.set_font_description(Some(&font_desc));
        let cancel_layout = pango::Layout::new(&pango_context);
        cancel_layout.set_font_description(Some(&font_desc));

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

        let mut ok_button = Button::new(config.ok_button.button, ok_label);
        let mut cancel_button = Button::new(config.cancel_button.button, cancel_label);
        balance_button_extents(&mut ok_button, &mut cancel_button);

        let mut indicator = match config.indicator.indicator_type {
            IndicatorType::Strings { strings } => {
                let strings_layout = pango::Layout::new(&pango_context);
                strings_layout.set_font_description(Some(&font_desc));
                Indicator::Strings(indicator::Strings::new(
                    config.indicator.common,
                    strings,
                    strings_layout,
                )?)
            }
            IndicatorType::Classic { classic } => Indicator::Classic(indicator::Classic::new(
                config.indicator.common,
                classic,
                text_height as f64,
            )),
            IndicatorType::Circle { circle } => Indicator::Circle(indicator::Circle::new(
                config.indicator.common,
                circle,
                text_height as f64,
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
            font_desc,
        };

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
            input_timeout_duration: config.input_timeout.map(Duration::from_secs),
            input_timeout: sleep(Duration::from_secs(config.input_timeout.unwrap_or(0))),
            debug,
        })
    }

    pub fn on_displayed(&mut self, serial: FrameId) -> bool {
        self.indicator.on_displayed(serial)
    }

    pub fn update(&mut self, cr: &cairo::Context, serial: FrameId) {
        self.indicator.update(cr, &self.background);
        trace!("update serial {:?}", serial);
        self.indicator.set_painted(serial);
        for (i, b) in self.buttons.iter_mut().enumerate() {
            if b.dirty {
                trace!("button {} dirty", i);
                b.clear(cr, &self.background);
                b.paint(cr)
            }
        }
    }

    pub fn window_size(&self, cr: &cairo::Context) -> (u16, u16) {
        let size = cr.user_to_device_distance(self.width, self.height);
        (size.0.round() as u16, size.1.round() as u16)
    }

    pub fn init(&mut self, cr: &cairo::Context, serial: FrameId) {
        // TODO can I preserve antialiasing without clearing the image first?
        cr.set_source(&self.background);
        cr.paint();
        self.paint(cr, serial);
    }

    fn paint(&mut self, cr: &cairo::Context, serial: FrameId) {
        trace!("paint");
        for l in &mut self.labels {
            l.paint(cr);
        }
        self.indicator.paint(cr);
        self.indicator.set_painted(serial);
        for b in &mut self.buttons {
            b.paint(cr);
        }
    }

    pub async fn run_events(mut self, xcontext: &mut XContext<'_>) -> Result<Option<Passphrase>> {
        loop {
            tokio::select! {
                _ = &mut self.input_timeout, if self.input_timeout_duration.is_some() => {
                    info!("input timeout");
                    return Ok(None)
                }
                updated = self.indicator.handle_events(), if self.indicator.requests_events() => {
                    if updated {
                        xcontext.backbuffer.update(&mut self)?;
                    }
                }
                // TODO without restarting wait_for_event on every loop
                event_res = xcontext.wait_for_event() => {
                    let event = event_res?;
                        match event {
                            Event::Motion { x, y } => {
                                if self.handle_motion(x, y) {
                                    xcontext.backbuffer.update(&mut self)?;
                                }
                            }
                            Event::KeyPress(key_press) => {
                                let (action, dirty) = self.handle_key_press(key_press, xcontext)?;
                                match action {
                                    Action::Ok => return Ok(Some(self.indicator.into_pass())),
                                    Action::Cancel => return Ok(None),
                                    Action::NoAction => {}
                                    _ => unreachable!(),
                                }
                                if dirty {
                                    xcontext.backbuffer.update(&mut self)?;
                                }
                            }
                            Event::ButtonPress { button, x, y, isrelease } => {
                                let (action, dirty) = self.handle_button_press(button, x, y, isrelease);
                                match action {
                                    Action::Ok => return Ok(Some(self.indicator.into_pass())),
                                    Action::Cancel => return Ok(None),
                                    Action::PastePrimary => {
                                        xcontext.paste_primary()?;
                                    }
                                    Action::PasteClipboard => {
                                        xcontext.paste_clipboard()?;
                                    }
                                    Action::PlainText => {
                                        self.indicator.toggle_plaintext();
                                        trace!("dirty {}", dirty);
                                    }
                                    Action::NoAction => {}
                                }
                                if dirty {
                                    xcontext.backbuffer.update(&mut self)?;
                                }
                            }
                            Event::Paste(mut val) => {
                                if self.paste(&val) {
                                    xcontext.backbuffer.update(&mut self)?;
                                }
                                val.zeroize();
                            }
                            Event::Focus(focus) => {
                                if self.indicator.set_focused(focus) {
                                    xcontext.backbuffer.update(&mut self)?;
                                }
                            }
                            Event::PendingUpdate => {
                                xcontext.backbuffer.update(&mut self)?;
                            }
                            Event::VsyncCompleted(serial) => {
                                if self.on_displayed(serial) {
                                    trace!("on displayed dirty");
                                    xcontext.backbuffer.update(&mut self)?;
                                }
                            }
                            Event::Exit => {
                                return Ok(None);
                            }
                        }
                }
            }
        }
    }

    pub fn handle_motion(&mut self, x: f64, y: f64) -> bool {
        let mut found = false;
        let mut dirty = false;
        for b in &mut self.buttons {
            if found {
                b.set_hover(false);
            } else if b.is_inside(x, y) {
                b.set_hover(true);
                found = true;
            } else {
                b.set_hover(false);
            }
            dirty = dirty || b.dirty;
        }
        dirty
    }

    fn cairo_context_changed(&mut self, cr: &cairo::Context) {
        for l in &mut self.labels {
            l.cairo_context_changed(&cr);
        }
        for b in &mut self.buttons {
            b.label.cairo_context_changed(&cr);
        }
    }

    pub fn resize(
        &mut self,
        cr: &cairo::Context,
        width: u16,
        height: u16,
        serial: FrameId,
        surface_cleared: bool,
    ) {
        cr.set_source(&self.background);

        if surface_cleared {
            // clear the whole buffer
            cr.paint();
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
            cr.fill();
        }

        let mut m = cr.get_matrix();

        let (dialog_width, dialog_height) = self.window_size(cr);
        m.x0 = if width > dialog_width {
            // floor to pixels
            ((width - dialog_width) / 2) as f64
        } else {
            0.0
        };
        m.y0 = if height > dialog_height {
            // floor to pixels
            ((height - dialog_height) / 2) as f64
        } else {
            0.0
        };

        cr.set_matrix(m);

        self.cairo_context_changed(&cr);

        self.paint(&cr, serial);
    }

    pub fn handle_button_press(
        &mut self,
        button: xproto::ButtonIndex,
        x: f64,
        y: f64,
        isrelease: bool,
    ) -> (Action, bool) {
        if !isrelease && button == xproto::ButtonIndex::M2 {
            self.mouse_middle_pressed = true;
        } else if self.mouse_middle_pressed && button == xproto::ButtonIndex::M2 {
            self.mouse_middle_pressed = false;
            if x >= 0.0 && x < self.width as f64 && y >= 0.0 && y < self.height as f64 {
                trace!("PRIMARY selection");
                return (Action::PastePrimary, false);
            }
        } else if button != xproto::ButtonIndex::M1 {
            trace!("not the left mouse button");
        } else {
            return self.handle_mouse_left_button_press(x, y, isrelease);
        }
        (Action::NoAction, false)
    }

    // Return true iff dialog should be repainted
    fn handle_mouse_left_button_press(&mut self, x: f64, y: f64, release: bool) -> (Action, bool) {
        if release {
            for (i, b) in self.buttons.iter_mut().enumerate() {
                if b.pressed {
                    b.set_pressed(false);
                    if b.is_inside(x, y) {
                        trace!("release inside button {}", i);
                        return (Components::ACTIONS[i], b.dirty);
                    } else {
                        return (Action::NoAction, b.dirty);
                    }
                }
            }
        } else {
            for (i, b) in self.buttons.iter_mut().enumerate() {
                if b.is_inside(x, y) {
                    trace!("inside button {}", i);
                    b.set_pressed(true);
                    return (Action::NoAction, b.dirty);
                }
            }
        }
        (Action::NoAction, false)
    }

    pub fn handle_key_press(
        &mut self,
        key_press: Keypress,
        xcontext: &mut XContext,
    ) -> Result<(Action, bool)> {
        let key = key_press.get_key();
        if keyboard::keysyms::XKB_KEY_Insert == xcontext.keyboard.key_get_one_sym(key)
            && xcontext.keyboard.mod_name_is_active(
                keyboard::names::XKB_MOD_NAME_SHIFT,
                keyboard::xkb_state_component::XKB_STATE_MODS_EFFECTIVE,
            )
        {
            xcontext.paste_primary()?;
            return Ok((Action::NoAction, false));
        }

        let mut dirty = false;
        let s = xcontext.keyboard.secure_key_get_utf8(key);
        if !s.unsecure().is_empty() {
            if let Some(timeout) = self.input_timeout_duration {
                self.input_timeout
                    .reset(Instant::now().checked_add(timeout).unwrap());
            }
            for letter in s.unsecure().chars() {
                if self.debug {
                    debug!("letter: {:?}", letter);
                } else {
                    debug!("letter");
                }
                let (action, d) = self.handle_utf8(letter, xcontext)?;
                dirty = dirty || d;
                if !matches!(action, Action::NoAction) {
                    return Ok((action, dirty));
                }
            }
        }
        Ok((Action::NoAction, dirty))
    }

    fn handle_utf8(&mut self, letter: char, xcontext: &mut XContext) -> Result<(Action, bool)> {
        let mut dirty = false;
        match letter {
            '\r' | '\n' => return Ok((Action::Ok, false)),
            '\x1b' => return Ok((Action::Cancel, false)),
            // backspace
            '\x08' | '\x7f' => {
                if self.indicator.pass.len > 0 {
                    self.indicator.pass.len -= 1;
                    dirty = self.indicator.passphrase_updated();
                }
            }
            // ctrl-u
            '\u{15}' => {
                if self.indicator.pass.len != 0 {
                    self.indicator.pass.len = 0;
                    dirty = self.indicator.passphrase_updated();
                }
            }
            // ctrl-v
            '\u{16}' => {
                xcontext.paste_clipboard()?;
            }
            l => {
                if self.indicator.pass.push(l).is_ok() {
                    dirty = self.indicator.passphrase_updated();
                }
            }
        }
        Ok((Action::NoAction, dirty))
    }

    pub fn paste(&mut self, s: &str) -> bool {
        let mut updated = false;
        for l in s.chars() {
            if self.indicator.pass.push(l).is_err() {
                break;
            }
            updated = true;
        }
        if updated {
            let dirty1 = self.indicator.passphrase_updated();
            let dirty2 = self.indicator.show_selection();
            dirty1 || dirty2
        } else {
            false
        }
    }
}
