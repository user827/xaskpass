use log::{debug, trace};
use serde::{Deserialize, Serialize};

use super::{Components, Indicator};
use crate::config;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Layout {
    BottomLeft,
    Center,
    MiddleCompact,
    TopRight,
}

impl Layout {
    pub fn get_fn(self) -> fn(&config::Layout, &mut Components, &mut Indicator) -> (f64, f64) {
        match self {
            Layout::TopRight => top_right,
            Layout::Center => center,
            Layout::BottomLeft => bottom_left,
            Layout::MiddleCompact => middle_compact,
        }
    }
}

pub fn bottom_left(
    config: &config::Layout,
    components: &mut Components,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing(components.text_height);
    indicator.for_width(components.ok().width);
    let buttonind_area_width = (4.0 * horizontal_spacing)
        + components.ok().width
        + components.cancel().width
        + indicator.width;
    components.label().calc_extents(config.text_width, true);
    let label_area_width = components.label().width + (2.0 * horizontal_spacing);
    let width = label_area_width.max(buttonind_area_width);
    // floor instead of round so these stay within the widths specified above
    let inter_buttonind_space = ((width - components.ok().width - indicator.width) / 3.0).floor();
    indicator.x = inter_buttonind_space;
    components.label().x = horizontal_spacing;
    components.ok().x = width - horizontal_spacing - components.ok().width;
    components.cancel().x = components.ok().x;

    let vertical_spacing = config.vertical_spacing(components.text_height);
    let button_area_height: f64 =
        vertical_spacing + components.ok().height + components.cancel().height;
    let buttonind_area_height = button_area_height.max(indicator.height);
    let space = vertical_spacing;
    let height =
        (2.0 * vertical_spacing) + components.label().height + buttonind_area_height + space;
    components.label().y = vertical_spacing;
    components.ok().y = components.label().y + components.label().height + space;
    indicator.y = components.ok().y
        + (height - components.ok().y - indicator.height - vertical_spacing) / 2.0;
    components.cancel().y = components.ok().y + components.ok().height + vertical_spacing;

    (width, height)
}

pub fn middle_compact(
    config: &config::Layout,
    components: &mut Components,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing(components.text_height);
    indicator.for_width(components.ok().width);
    let buttonind_area_width = (8.0 * horizontal_spacing)
        + components.ok().width
        + components.cancel().width
        + indicator.width;
    components.label().calc_extents(config.text_width, true);
    let label_area_width = components.label().width + (2.0 * horizontal_spacing);
    let width = label_area_width.max(buttonind_area_width);
    components.label().x = (width - components.label().width) / 2.0;
    let inter_space =
        (width - components.ok().width - components.cancel().width - indicator.width) / 4.0;
    components.ok().x = inter_space;
    indicator.x = components.ok().x + components.ok().width + inter_space;
    components.cancel().x = indicator.x + indicator.width + inter_space;

    let vertical_spacing = config.vertical_spacing(components.text_height);
    let buttonind_area_height = components
        .ok()
        .height
        .max(components.cancel().height)
        .max(indicator.height);
    let height = (3.0 * vertical_spacing) + components.label().height + buttonind_area_height;
    components.label().y = vertical_spacing;
    components.ok().y = height - vertical_spacing - components.ok().height;
    components.cancel().y = components.ok().y;
    indicator.y = height - vertical_spacing - buttonind_area_height
        + (buttonind_area_height - indicator.height) / 2.0;
    trace!(
        "buttonind_area_height: {}, indicator.height: {}, components.ok().height: {}",
        buttonind_area_height,
        indicator.height,
        components.ok().height
    );
    (width, height)
}

// TODO mess
pub fn center(
    config: &config::Layout,
    components: &mut Components,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing(components.text_height);
    let button_area_width =
        (3.0 * horizontal_spacing) + components.ok().width + components.cancel().width;
    components.label().calc_extents(config.text_width, true);
    let label_area_width = components.label().width + (2.0 * horizontal_spacing);
    let w = label_area_width.max(button_area_width);
    let indicator_spacing = (components.text_height / 4.0).round();
    debug!("layout indicator_spacing: {}", indicator_spacing);
    let indicator_label_space = if matches!(indicator, Indicator::Circle(..)) {
        components.indicator_label().calc_extents(None, false);
        components.indicator_label().width + indicator_spacing
    } else {
        0.0
    };
    let mut for_width = w
        - 2.0 * horizontal_spacing
        - components.clipboard().width
        - indicator_spacing
        - indicator_label_space;
    if indicator.has_plaintext() {
        for_width -= components.plaintext().width + indicator_spacing;
    }
    indicator.for_width(for_width);
    let mut indicator_area_width = indicator.width
        + 2.0 * horizontal_spacing
        + components.clipboard().width
        + indicator_spacing
        + indicator_label_space;

    if indicator.has_plaintext() {
        indicator_area_width += components.plaintext().width + indicator_spacing;
    }
    let width = w.max(indicator_area_width);

    let indicator_label_x =
        ((width - indicator_area_width + horizontal_spacing * 2.0) / 2.0).floor();
    if matches!(indicator, Indicator::Circle(..)) {
        components.indicator_label().x = indicator_label_x;
    }
    indicator.x = indicator_label_x + indicator_label_space;
    components.clipboard().x = indicator.x + indicator.width + indicator_spacing;
    if indicator.has_plaintext() {
        components.plaintext().x =
            components.clipboard().x + components.clipboard().width + indicator_spacing;
    }
    // floor instead of round so these stay within the widths specified above
    components.label().x = ((width - components.label().width) / 2.0).floor();
    let inter_button_space =
        ((width - components.ok().width - components.cancel().width) / 3.0).floor();
    components.ok().x = inter_button_space;
    components.cancel().x = components.ok().x + components.ok().width + inter_button_space;

    let vertical_spacing = config.vertical_spacing(components.text_height);
    let mut indicator_area_height = if matches!(indicator, Indicator::Circle(..)) {
        indicator
            .height
            .max(components.clipboard().height)
            .max(components.indicator_label().height)
    } else {
        indicator.height.max(components.clipboard().height)
    };
    if indicator.has_plaintext() {
        indicator_area_height = indicator_area_height.max(components.plaintext().height);
    }
    let height = (4.0 * vertical_spacing)
        + components.label().height
        + indicator_area_height
        + components.ok().height;

    components.label().y = vertical_spacing;
    let indicator_area_y = components.label().y + components.label().height + vertical_spacing;
    if matches!(indicator, Indicator::Circle(..)) {
        components.indicator_label().y = indicator_area_y
            + ((indicator_area_height - components.indicator_label().height) / 2.0).floor();
    }
    indicator.y = indicator_area_y + ((indicator_area_height - indicator.height) / 2.0).floor();
    components.clipboard().y =
        indicator_area_y + ((indicator_area_height - components.clipboard().height) / 2.0).floor();
    if indicator.has_plaintext() {
        components.plaintext().y = indicator_area_y
            + ((indicator_area_height - components.plaintext().height) / 2.0).floor();
    }
    components.ok().y = indicator_area_y + indicator_area_height + vertical_spacing;
    components.cancel().y = components.ok().y;

    (width, height)
}

pub fn top_right(
    config: &config::Layout,
    components: &mut Components,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing(components.text_height);
    // debug!("label() width {}", label().width);
    let h_space = 2.0 * horizontal_spacing;
    indicator.for_width(components.ok().width);
    components.label().calc_extents(config.text_width, true);
    let label_area_width =
        components.label().width + (4.0 * horizontal_spacing) + indicator.width + h_space;
    // debug!("label() area width {}", label());
    let button_area_width =
        (3.0 * horizontal_spacing) + components.ok().width + components.cancel().width;
    let width = label_area_width.max(button_area_width);
    components.label().x = horizontal_spacing * 2.0;
    indicator.x = width - horizontal_spacing * 2.0 - indicator.width;
    components.ok().x = width - horizontal_spacing - components.ok().width;
    components.cancel().x = components.ok().x - horizontal_spacing - components.cancel().width;

    let vertical_spacing = config.vertical_spacing(components.text_height);
    let label_area_height = components.label().height.max(indicator.height);
    let v_space = 3.0 * vertical_spacing;
    let height = (2.0 * vertical_spacing) + label_area_height + components.ok().height + v_space;
    components.label().y = vertical_spacing;
    indicator.y = components.label().y;
    components.ok().y = components.label().y + label_area_height + v_space;
    components.cancel().y = components.ok().y;

    (width, height)
}
