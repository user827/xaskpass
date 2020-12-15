use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::ffi::CStr;
use std::ops::{Deref, DerefMut};

use libc::LC_ALL;
use log::{debug, log_enabled, trace, warn};
use pango::FontExt as _;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::xcb_ffi::XCBConnection;

use crate::config;
use crate::config::{IndicatorType, Rgba};
use crate::errors::X11ErrorString as _;

pub mod indicator;
pub mod layout;

pub struct Components {
    clipboard_config: Option<config::ClipboardButton>,
    buttons: Vec<Button>,
    text_height: u32,
}

impl Components {
    const ACTIONS: [Action; 3] = [Action::Ok, Action::Cancel, Action::PasteClipboard];

    fn ok(&mut self) -> &mut Button {
        &mut self.buttons[0]
    }

    fn cancel(&mut self) -> &mut Button {
        &mut self.buttons[1]
    }

    fn clipboard(&mut self) -> &mut Button {
        if self.buttons.get_mut(2).is_none() {
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
    pub fn paint(&mut self, cr: &cairo::Context) {
        match self {
            Self::Strings(i) => i.paint(cr),
            Self::Circle(i) => i.paint(cr),
            Self::Classic(i) => i.paint(cr),
        }
    }

    pub fn update(&mut self, cr: &cairo::Context, bg: &Pattern) {
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
                width: (text_height * 0.8).round(),
            },
            foreground,
        }
    }
    pub fn paint(&self, cr: &cairo::Context) {
        cr.save();
        cr.translate(self.rectangle.x, self.rectangle.y);

        let line_width = 1.0;
        let small_distance = (self.rectangle.width / 10.0).round().max(1.0);
        let small_height = ((self.rectangle.width - 4.0 * small_distance - 2.0 * line_width) * 0.6)
            .round()
            .max(2.0);
        cr.rectangle(0.0, 0.0, self.rectangle.width, self.rectangle.height);
        cr.rectangle(
            line_width + small_distance,
            0.0,
            self.rectangle.width - 2.0 * (small_distance + line_width),
            small_height,
        );
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.clip();

        let y_offset = (small_height / 2.0).floor();
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
        let small_width = self.rectangle.width - 4.0 * small_distance - 3.0 * line_width;
        cr.rectangle(
            line_width + small_distance * 2.0 + line_width / 2.0,
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
        trace!("(dx, dy): {:?}", cr.user_to_device(x + radius_x, y));
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
pub struct Dialog<'a> {
    background: Pattern,
    label: Label,
    buttons: Vec<Button>,
    pub indicator: Indicator,
    width: f64,
    height: f64,
    cr: cairo::Context,
    pub surface: XcbSurface<'a>,
    pub resize_requested: Option<(u16, u16)>,
}

impl<'a> Dialog<'a> {
    pub fn new(
        config: config::Dialog,
        screen: &xproto::Screen,
        surface: XcbSurface<'a>,
        label: Option<&str>,
    ) -> crate::errors::Result<Self> {
        let cr = cairo::Context::new(&surface);
        let context = pangocairo::create_context(&cr).unwrap();

        let dpi = if let Some(dpi) = config.dpi {
            dpi
        } else {
            (screen.height_in_pixels as f64 * 25.4 / screen.height_in_millimeters as f64)
                .max(96.0)
                .round()
        };
        debug!("dpi {}", dpi);
        pangocairo::context_set_resolution(&context, dpi);

        let language = pango::Language::default();
        debug!("language {}", language.to_string());
        context.set_language(&language);

        let font = config.font;
        let font_desc = pango::FontDescription::from_string(&font);

        debug!("font request: {}", font_desc.to_string());
        if log_enabled!(log::Level::Debug) {
            let closest_font = context
                .load_font(&font_desc)
                .unwrap()
                .describe()
                .map(|f| f.to_string())
                .unwrap_or_else(|| "<no name>".into());
            debug!("closest font: {}", closest_font);
        }

        let label_layout = pango::Layout::new(&context);
        label_layout.set_font_description(Some(&font_desc));
        label_layout.set_text(label.unwrap_or(&config.label));
        let (_, text_height) = label_layout.get_pixel_size();
        let text_height: u32 = text_height.try_into().unwrap();

        let mut label = Label::TextLabel(TextLabel::new(config.foreground.into(), label_layout));

        let ok_layout = pango::Layout::new(&context);
        ok_layout.set_font_description(Some(&font_desc));
        let cancel_layout = pango::Layout::new(&context);
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

        let mut buttons = Vec::with_capacity(3);
        buttons.push(ok_button);
        buttons.push(cancel_button);
        let mut components = Components {
            clipboard_config: Some(config.clipboard_button),
            buttons,
            text_height,
        };

        let mut indicator = match config.indicator.indicator_type {
            IndicatorType::Strings { strings } => {
                let strings_layout = pango::Layout::new(&context);
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

        let (width, height) = config.layout_opts.layout.get_fn()(
            &config.layout_opts,
            &mut label,
            &mut components,
            &mut indicator,
        );

        let mut buttons = components.buttons;

        for b in &mut buttons {
            b.calc_label_position();
        }

        Ok(Self {
            label,
            indicator,
            buttons,
            width,
            height,
            cr,
            surface,
            resize_requested: None,
            background: config.background.into(),
        })
    }

    pub fn window_size(&self) -> (u16, u16) {
        let size = self.cr.user_to_device_distance(self.width, self.height);
        (size.0 as u16, size.1 as u16)
    }

    pub fn update(&mut self) -> Result<(), crate::Error> {
        if let Some((width, height)) = self.resize_requested {
            trace!("resize requested");
            self.resize(width, height)?;
            self.resize_requested = None;
        } else {
            self.indicator.update(&self.cr, &self.background);
            for (i, b) in self.buttons.iter_mut().enumerate() {
                if b.dirty {
                    trace!("button {} dirty", i);
                    b.clear(&self.cr, &self.background);
                    b.paint(&self.cr)
                }
            }
        }
        self.surface.flush();
        Ok(())
    }

    fn resize(&mut self, width: u16, height: u16) -> Result<(), crate::Error> {
        self.cr.set_source(&self.background);

        if self.surface.resize(width, height)? {
            // clear the whole buffer
            self.cr.paint();
        } else {
            // use the translation matrix for the previous window size to clear the previously used
            // area
            self.cr.rectangle(0.0, 0.0, self.width, self.height);
            self.cr.fill();
        }

        let mut m = self.cr.get_matrix();

        // Scale isn't applid when directly accessing the matrix so no need to translate device to
        // user coordinates
        let (dialog_width, dialog_height) = self.window_size();
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

        self.cr.set_matrix(m);

        self.label.cairo_context_changed(&self.cr);
        for b in &mut self.buttons {
            b.label.cairo_context_changed(&self.cr);
        }

        self.paint();

        Ok(())
    }

    pub fn init(&mut self) {
        // TODO can I preserve antialiasing without clearing the image first?
        self.cr.set_source(&self.background);
        self.cr.paint();
        self.paint();
        self.surface.flush();
    }

    fn paint(&mut self) {
        let cr = &self.cr;
        trace!("matrix: {:?}", cr.get_matrix());
        self.label.paint(cr);
        self.indicator.paint(cr);
        for b in &mut self.buttons {
            b.paint(cr);
        }
    }

    pub fn handle_motion(&mut self, x: i16, y: i16) -> bool {
        let (x, y) = self.cr.device_to_user(x as f64, y as f64);
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

    // Return true iff dialog should be repainted
    pub fn handle_button_press(&mut self, dx: i16, dy: i16, release: bool) -> (Action, bool) {
        let (x, y) = self.cr.device_to_user(dx as f64, dy as f64);
        trace!("device_x: {}, device_y: {}, x: {}, y: {}", dx, dy, x, y);

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
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    NoAction,
    Ok,
    Cancel,
    PasteClipboard,
}

impl<'a> Drop for Dialog<'a> {
    fn drop(&mut self) {
        debug!("dropping Dialog");
    }
}

#[allow(non_camel_case_types)]
pub type xcb_visualid_t = u32;

#[derive(Debug, Clone, Copy)]
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct xcb_visualtype_t {
    pub visual_id: xcb_visualid_t,
    pub class: u8,
    pub bits_per_rgb_value: u8,
    pub colormap_entries: u16,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub pad0: [u8; 4],
}

impl From<xproto::Visualtype> for xcb_visualtype_t {
    fn from(value: xproto::Visualtype) -> Self {
        Self {
            visual_id: value.visual_id,
            class: value.class.into(),
            bits_per_rgb_value: value.bits_per_rgb_value,
            colormap_entries: value.colormap_entries,
            red_mask: value.red_mask,
            green_mask: value.green_mask,
            blue_mask: value.blue_mask,
            pad0: [0; 4],
        }
    }
}

#[derive(Debug)]
pub struct XcbSurface<'a> {
    conn: &'a crate::Connection,
    pub pixmap: xproto::Pixmap,
    surface: cairo::XCBSurface,
    width: u16,
    height: u16,
    drawable: xproto::Drawable,
    depth: u8,
}

impl<'a> XcbSurface<'a> {
    pub fn new(
        conn: &'a crate::Connection,
        drawable: xproto::Drawable,
        depth: u8,
        visual_type: &xproto::Visualtype,
        width: u16,
        height: u16,
    ) -> Result<Self, crate::Error> {
        let pixmap = conn.generate_id().map_xerr(conn)?;
        conn.create_pixmap(depth, pixmap, drawable, width, height)?;

        let surface = Self::create(conn, pixmap, visual_type, width, height);

        Ok(Self {
            conn,
            surface,
            pixmap,
            drawable,
            height,
            width,
            depth,
        })
    }

    pub fn create(
        conn: &XCBConnection,
        drawable: xproto::Drawable,
        visual_type: &xproto::Visualtype,
        width: u16,
        height: u16,
    ) -> cairo::XCBSurface {
        let cairo_conn =
            unsafe { cairo::XCBConnection::from_raw_none(conn.get_raw_xcb_connection() as _) };
        let mut xcb_visualtype: xcb_visualtype_t = (*visual_type).into();
        let cairo_visual =
            unsafe { cairo::XCBVisualType::from_raw_none(&mut xcb_visualtype as *mut _ as _) };
        let cairo_drawable = cairo::XCBDrawable(drawable);
        cairo::XCBSurface::create(
            &cairo_conn,
            &cairo_drawable,
            &cairo_visual,
            width.into(),
            height.into(),
        )
        .unwrap()
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Result<bool, crate::Error> {
        if width <= self.width && height <= self.height {
            return Ok(false);
        }
        let mut new_width = self.width;
        let mut new_height = self.height;
        debug!("resizing");
        if width > new_width {
            new_width *= 2;
            if width > new_width {
                new_width = width;
            }
        }
        if height > new_height {
            new_height *= 2;
            if height > new_height {
                new_height = height;
            }
        }

        self.setup_pixmap(self.drawable, new_width, new_height)?;
        Ok(true)
    }

    pub fn setup_pixmap(
        &mut self,
        drawable: xproto::Drawable,
        new_width: u16,
        new_height: u16,
    ) -> Result<(), crate::Error> {
        let pixmap = self.conn.generate_id().map_xerr(self.conn)?;
        self.conn
            .create_pixmap(self.depth, pixmap, drawable, new_width, new_height)?;

        let cairo_pixmap = cairo::XCBDrawable(pixmap);
        self.surface
            .set_drawable(&cairo_pixmap, new_width.into(), new_height.into())
            .unwrap();
        self.conn.free_pixmap(self.pixmap)?;
        self.pixmap = pixmap;

        self.width = new_width;
        self.height = new_height;
        Ok(())
    }
}

impl<'a> Drop for XcbSurface<'a> {
    fn drop(&mut self) {
        debug!("dropping xcb surface");
        self.surface.finish();
        if let Err(err) = self.conn.free_pixmap(self.pixmap) {
            debug!("free pixmap failed: {}", err);
        }
    }
}

impl<'a> Deref for XcbSurface<'a> {
    type Target = cairo::XCBSurface;
    fn deref(&self) -> &Self::Target {
        &self.surface
    }
}
