use log::trace;
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
    pub fn get_fn(
        &self,
    ) -> fn(&config::Layout, &mut Components, &mut Indicator) -> (f64, f64) {
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
    let horizontal_spacing: f64 = config.horizontal_spacing;
    indicator.for_width(components.ok().width);
    let buttonind_area_width = (4.0 * horizontal_spacing)
        + components.ok().width
        + components.cancel().width
        + indicator.width;
    components.label().calc_extents(
        Some(
            config
                .text_width
                .unwrap_or(buttonind_area_width.round() as u32),
        ),
        true,
    );
    let label_area_width = components.label().width + (2.0 * horizontal_spacing);
    let width = label_area_width.max(buttonind_area_width);
    // floor instead of round so these stay within the widths specified above
    let inter_buttonind_space = ((width - components.ok().width - indicator.width) / 3.0).floor();
    indicator.x = inter_buttonind_space;
    components.label().x = horizontal_spacing;
    components.ok().x = width - horizontal_spacing - components.ok().width;
    components.cancel().x = components.ok().x;

    let vertical_spacing = config.vertical_spacing;
    let button_area_height: f64 =
        vertical_spacing + components.ok().height + components.cancel().height;
    let buttonind_area_height = button_area_height.max(indicator.height);
    let space = vertical_spacing;
    let height = (2.0 * vertical_spacing) + components.label().height + buttonind_area_height + space;
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
    let horizontal_spacing: f64 = config.horizontal_spacing;
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

    let vertical_spacing = config.vertical_spacing;
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

pub fn center(
    config: &config::Layout,
    components: &mut Components,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing = config.horizontal_spacing;
    let button_area_width =
        (3.0 * horizontal_spacing) + components.ok().width + components.cancel().width;
    components.label().calc_extents(config.text_width, true);
    let label_area_width = components.label().width + (2.0 * horizontal_spacing);
    let w = label_area_width.max(button_area_width);
    let indicator_spacing = 4.0;
    indicator
        .for_width(w - 2.0 * horizontal_spacing - components.clipboard().width - indicator_spacing);
    let indicator_area_width = indicator.width
        + 2.0 * horizontal_spacing
        + components.clipboard().width
        + indicator_spacing;
    let width = w.max(indicator_area_width);

    indicator.x = ((width - indicator.width) / 2.0).floor();
    if indicator.x < components.clipboard().width + horizontal_spacing + indicator_spacing {
        components.clipboard().x = width - components.clipboard().width - horizontal_spacing;
        indicator.x = components.clipboard().x - indicator_spacing - indicator.width;
    } else {
        components.clipboard().x = indicator.x + indicator.width + indicator_spacing;
    }
    // floor instead of round so these stay within the widths specified above
    components.label().x = ((width - components.label().width) / 2.0).floor();
    let inter_button_space =
        ((width - components.ok().width - components.cancel().width) / 3.0).floor();
    components.ok().x = inter_button_space;
    components.cancel().x = components.ok().x + components.ok().width + inter_button_space;

    let vertical_spacing = config.vertical_spacing;
    let indicator_area_height = indicator.height.max(components.clipboard().height);
    let height =
        (4.0 * vertical_spacing) + components.label().height + indicator_area_height + components.ok().height;

    components.label().y = vertical_spacing;
    let indicator_area_y = components.label().y + components.label().height + vertical_spacing;
    indicator.y = indicator_area_y + ((indicator_area_height - indicator.height) / 2.0).floor();
    components.clipboard().y =
        indicator_area_y + ((indicator_area_height - components.clipboard().height) / 2.0).floor();
    components.ok().y = indicator_area_y + indicator_area_height + vertical_spacing;
    components.cancel().y = components.ok().y;

    (width, height)
}

pub fn top_right(
    config: &config::Layout,
    components: &mut Components,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing;
    // debug!("label() width {}", label().width);
    let hspace = 2.0 * horizontal_spacing;
    indicator.for_width(components.ok().width);
    let text_width = config
        .text_width
        .unwrap_or((components.ok().width + components.cancel().width).round() as u32);
    components.label().calc_extents(Some(text_width), true);
    let label_area_width = components.label().width + (4.0 * horizontal_spacing) + indicator.width + hspace;
    // debug!("label() area width {}", label());
    let button_area_width =
        (3.0 * horizontal_spacing) + components.ok().width + components.cancel().width;
    let width = label_area_width.max(button_area_width);
    components.label().x = horizontal_spacing * 2.0;
    indicator.x = width - horizontal_spacing * 2.0 - indicator.width;
    components.ok().x = width - horizontal_spacing - components.ok().width;
    components.cancel().x = components.ok().x - horizontal_spacing - components.cancel().width;

    let vertical_spacing = config.vertical_spacing;
    let label_area_height = components.label().height.max(indicator.height);
    let vspace = 3.0 * vertical_spacing;
    let height = (2.0 * vertical_spacing) + label_area_height + components.ok().height + vspace;
    components.label().y = vertical_spacing;
    indicator.y = components.label().y;
    components.ok().y = components.label().y + label_area_height + vspace;
    components.cancel().y = components.ok().y;

    (width, height)
}
